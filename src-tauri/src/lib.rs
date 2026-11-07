use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}, io::{Read, BufReader, Write}};
use sha2::{Digest, Sha256};
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String, name: String, slug: String, description: String,
    status: String, tags: Vec<String>, created_at: String, updated_at: String,
    archived: bool, file_count: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectInput { name: String, description: String, status: String, tags: Vec<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFile { relative_path: String, size_bytes: u64, extension: String }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileChange { relative_path: String, kind: String, size_bytes: u64 }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanResult { changes: Vec<FileChange>, baseline_created: bool }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionInfo { id: String, project_id: String, label: String, description: String, created_at: String, file_count: i64, total_size_bytes: i64 }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionFile { relative_path: String, size_bytes: i64, sha256: String }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionResult { version: VersionInfo, files: Vec<VersionFile> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct FileRevision { id: String, relative_path: String, sha256: String, size_bytes: i64, captured_at: String, source: String }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct FileImportResult { imported: Vec<String>, replaced: Vec<String>, unchanged: Vec<String> }


const AUDIT_COPY_MARKER: &str = ".3db-audit10-copy.marker";
const AUDIT_COPY_MARKER_VALUE: &str = "3db-audit10-isolated-copy-v1\n";

fn production_root() -> Result<PathBuf, String> {
    let base = dirs::document_dir().or_else(dirs::home_dir)
        .ok_or("Kein Dokumenten- oder Benutzerverzeichnis gefunden")?;
    Ok(base.join("3db"))
}

// Audit-only opt-in: the normal application still uses the legacy data location.
// Never redirect to a production folder, its ancestors/descendants, or an unmarked folder.
fn validate_audit_copy(candidate: &Path, production: &Path) -> Result<PathBuf, String> {
    if !candidate.is_absolute() { return Err("Audit-Pfad muss absolut sein".into()); }
    let path = candidate.canonicalize().map_err(|e| format!("Audit-Ordner nicht gefunden: {e}"))?;
    if !path.is_dir() { return Err("Audit-Ziel ist kein Ordner".into()); }
    let marker = path.join(AUDIT_COPY_MARKER);
    if fs::read_to_string(&marker).map_err(|_| "Audit-Markierung fehlt".to_string())?
        != AUDIT_COPY_MARKER_VALUE { return Err("Ungültige Audit-Markierung".into()); }
    if !path.join(".3db").join("3db.db").is_file() {
        return Err("Audit-Kopie besitzt keine bestehende SQLite-Datenbank".into());
    }
    if !path.join("Projects").is_dir() { return Err("Audit-Kopie besitzt keinen Projects-Ordner".into()); }
    // Canonical paths prevent escaping a known production root by aliases/junctions.
    let prod = production.canonicalize().unwrap_or_else(|_| production.to_path_buf());
    if path == prod || path.starts_with(&prod) || prod.starts_with(&path) {
        return Err("Audit-Pfad überlappt mit dem produktiven 3db-Ordner".into());
    }
    // Explicitly forbid using either the user's home or Documents directory itself.
    if let Some(home) = dirs::home_dir() {
        if let Ok(home) = home.canonicalize() {
            if path == home || home.starts_with(&path) {return Err("Audit-Pfad überlappt mit dem Benutzerverzeichnis".into());}
        }
    }
    Ok(path)
}

fn root() -> Result<PathBuf, String> {
    if let Ok(override_path) = std::env::var("THREEDB_AUDIT_ROOT") {
        if std::env::var("THREEDB_AUDIT_CONFIRM").as_deref() != Ok("ISOLATED_COPY_ONLY") {
            return Err("Audit-Pfad gesetzt, aber Freigabe fehlt; Start abgebrochen".into());
        }
        return validate_audit_copy(Path::new(&override_path), &production_root()?);
    }
    production_root()
}
fn setup_root() -> Result<PathBuf, String> {
    let root = root()?;
    fs::create_dir_all(root.join("Projects")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join(".3db")).map_err(|e| e.to_string())?;
    Ok(root)
}
fn initialize_database_schema(conn: &mut Connection) -> Result<(), String> {
    // Foreign keys and WAL journal mode are connection configuration, not part of the migration transaction.
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
        .map_err(|e| e.to_string())?;
    // Schema creation and historical snapshot backfill commit together, or both roll back.
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute_batch("
        CREATE TABLE IF NOT EXISTS projects (
          id TEXT PRIMARY KEY,
          name TEXT NOT NULL,
          slug TEXT NOT NULL UNIQUE,
          description TEXT NOT NULL DEFAULT '',
          status TEXT NOT NULL CHECK(status IN ('idea','working','testing','finished')),
          tags_json TEXT NOT NULL DEFAULT '[]',
          archived INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status);
        CREATE INDEX IF NOT EXISTS idx_projects_updated ON projects(updated_at);
        CREATE TABLE IF NOT EXISTS project_details (
          project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
          category TEXT NOT NULL DEFAULT '',
          author TEXT NOT NULL DEFAULT '',
          license TEXT NOT NULL DEFAULT '',
          source_url TEXT NOT NULL DEFAULT '',
          updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_project_details_category ON project_details(category);
        CREATE TABLE IF NOT EXISTS completeness_rules (
          id TEXT PRIMARY KEY,
          project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
          label TEXT NOT NULL,
          rule_type TEXT NOT NULL CHECK(rule_type IN ('stl','3mf','image','pdf','task')),
          required INTEGER NOT NULL DEFAULT 1 CHECK(required IN (0,1)),
          done INTEGER NOT NULL DEFAULT 0 CHECK(done IN (0,1)),
          position INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_completeness_project ON completeness_rules(project_id,position);
        CREATE TABLE IF NOT EXISTS global_file_requirements (
          kind TEXT PRIMARY KEY CHECK(kind IN ('stl','3mf','image','pdf')),
          required INTEGER NOT NULL CHECK(required IN (0,1))
        );
        CREATE TABLE IF NOT EXISTS file_baselines (
          project_id TEXT NOT NULL REFERENCES projects(id),
          relative_path TEXT NOT NULL,
          sha256 TEXT NOT NULL,
          size_bytes INTEGER NOT NULL,
          PRIMARY KEY(project_id,relative_path)
        );
        CREATE TABLE IF NOT EXISTS project_scan_state (
          project_id TEXT PRIMARY KEY REFERENCES projects(id),
          initialized_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS snapshot_objects (
          sha256 TEXT PRIMARY KEY,
          size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS project_versions (
          id TEXT PRIMARY KEY,
          project_id TEXT NOT NULL REFERENCES projects(id),
          label TEXT NOT NULL,
          description TEXT NOT NULL DEFAULT '',
          created_at TEXT NOT NULL,
          metadata_json TEXT NOT NULL,
          file_count INTEGER NOT NULL,
          total_size_bytes INTEGER NOT NULL,
          UNIQUE(project_id, label)
        );
        CREATE INDEX IF NOT EXISTS idx_project_versions_project ON project_versions(project_id,created_at DESC);
        CREATE TABLE IF NOT EXISTS file_revisions (
          id TEXT PRIMARY KEY,
          project_id TEXT NOT NULL REFERENCES projects(id),
          relative_path TEXT NOT NULL,
          sha256 TEXT NOT NULL REFERENCES snapshot_objects(sha256),
          size_bytes INTEGER NOT NULL,
          captured_at TEXT NOT NULL,
          source TEXT NOT NULL,
          UNIQUE(project_id,relative_path,sha256)
        );
        CREATE INDEX IF NOT EXISTS idx_file_revisions ON file_revisions(project_id,relative_path,captured_at DESC);
        CREATE TABLE IF NOT EXISTS variants (
          id TEXT PRIMARY KEY,
          project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
          name TEXT NOT NULL COLLATE NOCASE,
          description TEXT NOT NULL DEFAULT '',
          created_at TEXT NOT NULL,
          UNIQUE(project_id,name),
          UNIQUE(project_id,id)
        );
        CREATE INDEX IF NOT EXISTS idx_variants_project ON variants(project_id);
        CREATE TABLE IF NOT EXISTS variant_files (
          project_id TEXT NOT NULL,
          variant_id TEXT NOT NULL,
          relative_path TEXT NOT NULL,
          PRIMARY KEY(project_id,variant_id,relative_path),
          FOREIGN KEY(project_id,variant_id) REFERENCES variants(project_id,id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_variant_files_project ON variant_files(project_id,relative_path);
        CREATE TABLE IF NOT EXISTS snapshot_files (
          version_id TEXT NOT NULL REFERENCES project_versions(id),
          relative_path TEXT NOT NULL,
          sha256 TEXT NOT NULL REFERENCES snapshot_objects(sha256),
          size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
          PRIMARY KEY(version_id, relative_path)
        );")
      .map_err(|e| e.to_string())?;
    // Existing 0.3 project snapshot file contents become visible in per-file history.
    tx.execute_batch("INSERT OR IGNORE INTO file_revisions(id,project_id,relative_path,sha256,size_bytes,captured_at,source)
        SELECT lower(hex(randomblob(16))),v.project_id,f.relative_path,f.sha256,f.size_bytes,v.created_at,'project_snapshot'
        FROM snapshot_files f JOIN project_versions v ON v.id=f.version_id WHERE f.relative_path <> 'project.json';")
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

fn database() -> Result<Connection, String> {
    let root = setup_root()?;
    let mut conn = Connection::open(root.join(".3db").join("3db.db"))
        .map_err(|e| e.to_string())?;
    initialize_database_schema(&mut conn)?;
    Ok(conn)
}
fn all_projects() -> Result<Vec<Project>, String> {
    let conn = database()?;
    let mut query = conn.prepare("SELECT id,name,slug,description,status,tags_json,archived,created_at,updated_at FROM projects ORDER BY updated_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = query.query_map([], |r| {
       let tags: String = r.get(5)?;
       Ok(Project { id:r.get(0)?, name:r.get(1)?, slug:r.get(2)?, description:r.get(3)?,
         status:r.get(4)?, tags:serde_json::from_str(&tags).unwrap_or_default(),
         archived:r.get::<_,i64>(6)?!=0, created_at:r.get(7)?, updated_at:r.get(8)?, file_count:0 })
    }).map_err(|e| e.to_string())?;
    let mut projects = Vec::new();
    for row in rows {
        let mut p = row.map_err(|e| e.to_string())?;
        p.file_count = walk_files(&setup_root()?.join("Projects").join(&p.slug))?.len();
        projects.push(p);
    }
    Ok(projects)
}
fn slugify(name: &str) -> String {
    let plain = deunicode::deunicode(name);
    let mut s = String::new();
    for ch in plain.chars() {
        if ch.is_ascii_alphanumeric() {s.push(ch);} else if !s.ends_with('-') {s.push('-');}
    }
    let slug = s.trim_matches('-');
    if slug.is_empty(){"Projekt".into()} else {slug.chars().take(60).collect()}
}
fn valid_input(input: &ProjectInput) -> Result<(), String> {
    if input.name.trim().is_empty() || input.name.chars().count()>120 {return Err("Projektname muss zwischen 1 und 120 Zeichen haben".into());}
    if !["idea","working","testing","finished"].contains(&input.status.as_str()) {return Err("Ungültiger Projektstatus".into());}
    if input.description.chars().count()>5000 {return Err("Beschreibung ist zu lang".into());}
    if input.tags.len()>40 || input.tags.iter().any(|t| t.len()>50) {return Err("Zu viele oder zu lange Tags".into());}
    Ok(())
}
// Install a fully written manifest without first deleting the only good copy.
// If the second rename fails, restore the previous file from its backup.
fn replace_manifest_safely(temp: &Path, dest: &Path) -> Result<(), String> {
    let backup = dest.with_file_name(format!("project.json.backup-{}", uuid::Uuid::new_v4()));
    let had_original = dest.exists();
    if had_original {
        fs::rename(dest, &backup).map_err(|e|format!("Altes Manifest konnte nicht gesichert werden: {e}"))?;
    }
    if let Err(e) = fs::rename(temp, dest) {
        if had_original {
            if let Err(restore_err) = fs::rename(&backup, dest) {
                return Err(format!("Manifest konnte nicht ersetzt werden ({e}); ALTE DATEI ALS BACKUP ERHALTEN: {} (Wiederherstellung fehlgeschlagen: {restore_err})", backup.display()));
            }
        }
        return Err(format!("Manifest konnte nicht ersetzt werden; alter Stand wiederhergestellt: {e}"));
    }
    if had_original {
        // An unsuccessful backup cleanup must never invalidate the new manifest.
        let _ = fs::remove_file(&backup);
    }
    Ok(())
}

fn write_manifest(dir: &Path, project: &Project) -> Result<(), String> {
    let old: serde_json::Value = fs::read(dir.join("project.json")).ok()
      .and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_else(||serde_json::json!({}));
    let manifest = serde_json::json!({
      "schemaVersion": 1, "projectId": project.id, "name":project.name,
      "slug":project.slug, "description":project.description, "status":project.status,
      "tags":project.tags, "createdAt":project.created_at, "updatedAt":project.updated_at,
      "archived":project.archived,
      "variants":old.get("variants").cloned().unwrap_or_else(||serde_json::json!([])),
      "variantFiles":old.get("variantFiles").cloned().unwrap_or_else(||serde_json::json!([])),
      "details":old.get("details").cloned().unwrap_or_else(||serde_json::json!({}))
    });
    let temp = dir.join("project.json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(&manifest).map_err(|e|e.to_string())?)
      .map_err(|e|e.to_string())?;
    let dest = dir.join("project.json");
    replace_manifest_safely(&temp, &dest)
}

// Import preview and commit use exactly the same deterministic filename mapping.
// Source directories are only read; all operations take place in the managed library.
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct ImportEntry {
    source_relative_path: String, target_relative_path: String,
    size_bytes: u64, category: String,
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct ImportPreview {
    suggested_name: String, entries: Vec<ImportEntry>,
    total_size_bytes: u64, conflicts: Vec<String>, skipped_links: usize,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct ImportMapping { source_relative_path: String, category: String, target_name: Option<String> }

fn source_tree(source_dir: &Path) -> Result<(Vec<(PathBuf,String,u64)>,usize),String> {
    const MAX_FILES:usize=3000;
    let mut files=Vec::new();let mut links=0usize;
    let mut stack=vec![source_dir.to_path_buf()];
    while let Some(dir)=stack.pop(){
        for entry in fs::read_dir(&dir).map_err(|e|format!("Quellordner nicht lesbar: {e}"))? {
            let entry=entry.map_err(|e|e.to_string())?;
            let kind=entry.file_type().map_err(|e|e.to_string())?;
            if kind.is_symlink(){links+=1;continue;}
            if kind.is_dir(){stack.push(entry.path());continue;}
            if !kind.is_file(){continue;}
            let relative=entry.path().strip_prefix(source_dir).map_err(|e|e.to_string())?
              .to_str().ok_or("Dateipfad ist nicht als UTF-8 darstellbar")?.replace('\\',"/").replace('\\',"/");
            let relative=relative.replace('\\',"/");
            if relative.split('/').any(|part| part==".3db" || part==".git") {continue;}
            let size=entry.metadata().map_err(|e|e.to_string())?.len();
            files.push((entry.path(),relative,size));
            if files.len()>MAX_FILES{return Err("Mehr als 3000 Dateien – bitte Import aufteilen".into());}
        }
    }
    files.sort_by(|a,b|a.1.cmp(&b.1));
    Ok((files,links))
}
fn canonical_source(source_dir:&str)->Result<PathBuf,String>{
    let dir=fs::canonicalize(source_dir).map_err(|e|format!("Quellordner nicht gefunden: {e}"))?;
    if !dir.is_dir(){return Err("Bitte einen Ordner auswählen".into());}
    let managed=fs::canonicalize(setup_root()?).map_err(|e|e.to_string())?;
    if dir.starts_with(&managed) || managed.starts_with(&dir) {
        return Err("Der Quellordner darf das 3db-Archiv weder enthalten noch darin liegen".into());
    }
    Ok(dir)
}
fn category_for_import(name:&str)->String {
    if name.eq_ignore_ascii_case("project.json") {"Other".into()} else {
        destination_for(name).unwrap_or("Other").into()
    }
}
fn validate_category(cat:&str)->Result<(),String>{
    if ["Models","Printfiles","Images","Documents","Other"].contains(&cat){Ok(())}
    else {Err("Ungültige Zielkategorie".into())}
}
fn import_plan(source_dir:&str,mappings:&[ImportMapping])->Result<(PathBuf,ImportPreview),String>{
    let base=canonical_source(source_dir)?;
    let (files,skipped_links)=source_tree(&base)?;
    if files.is_empty(){return Err("Der Ordner enthält keine importierbaren Dateien".into());}
    let mut selections=std::collections::HashMap::new();
    for m in mappings {
        validate_category(&m.category)?;
        if selections.insert(m.source_relative_path.clone(),(m.category.clone(),m.target_name.clone())).is_some(){return Err("Doppelte Zuordnung".into());}
    }
    let mut targets=std::collections::HashSet::new();
    let mut conflicts=Vec::new();let mut entries=Vec::new();let mut total=0u64;
    for (_,rel,size) in files {
        let path=Path::new(&rel);
        let name=path.file_name().and_then(|n|n.to_str()).ok_or("Ungültiger Dateiname")?;
        let category=selections.get(&rel).map(|v|v.0.clone()).unwrap_or_else(||category_for_import(name));
        let selected_name=selections.get(&rel).and_then(|v|v.1.as_deref()).unwrap_or(name);
        if selected_name.trim().is_empty() || selected_name=="." || selected_name==".." ||
           selected_name.chars().any(|ch| "\\/:*?\"<>|".contains(ch) || ch.is_control()) ||
           selected_name.ends_with('.') || selected_name.ends_with(' ') ||
           selected_name.eq_ignore_ascii_case("project.json") && category!="Other" {
            return Err(format!("Ungültiger oder reservierter Zieldateiname: {selected_name}"));
        }
        let target_name=if selected_name.eq_ignore_ascii_case("project.json") {"source-project.json"}else{selected_name};
        let target=format!("{category}/{target_name}");
        if !targets.insert(target.to_lowercase()) {conflicts.push(format!("Zielname mehrfach vorhanden: {target}"));}
        total=total.checked_add(size).ok_or("Projektgröße zu groß")?;
        entries.push(ImportEntry{source_relative_path:rel,target_relative_path:target,size_bytes:size,category});
    }
    if selections.keys().any(|k|!entries.iter().any(|e|&e.source_relative_path==k)) {
        return Err("Die Dateizuordnung ist veraltet; bitte erneut analysieren".into());
    }
    let suggested_name=base.file_name().and_then(|x|x.to_str()).unwrap_or("Importiertes Projekt").to_owned();
    Ok((base,ImportPreview{suggested_name,entries,total_size_bytes:total,conflicts,skipped_links}))
}
#[tauri::command]
fn preview_project_import(source_dir:String,mappings:Vec<ImportMapping>)->Result<ImportPreview,String>{
    Ok(import_plan(&source_dir,&mappings)?.1)
}
// Import ownership marker: only remove directories created by this import.
// On a crash after publishing, the marker deliberately remains for manual recovery.
const IMPORT_OWNER_FILE: &str = ".3db-import-owner";

fn owned_by_import(dir: &Path, token: &str) -> bool {
    fs::read_to_string(dir.join(IMPORT_OWNER_FILE))
        .map(|value| value == token)
        .unwrap_or(false)
}

fn publish_staged_import<F>(staging: &Path, final_path: &Path, token: &str, persist: F) -> Result<(), String>
where F: FnOnce() -> Result<(), String> {
    if !owned_by_import(staging, token) {
        return Err("Import-Staging hat keine gültige Eigentumskennung".into());
    }
    // This guard catches normal name collisions; a concurrent external writer
    // between this check and rename remains a release blocker (AUD-004).
    if final_path.exists() {
        return Err("Zielordner existiert bereits; keine Dateien überschrieben".into());
    }
    fs::rename(staging, final_path)
        .map_err(|e| format!("Import konnte nicht veröffentlicht werden: {e}"))?;
    match persist() {
        Ok(()) => {
            // Marker has no role after a successful DB commit.
            let _ = fs::remove_file(final_path.join(IMPORT_OWNER_FILE));
            Ok(())
        }
        Err(db_error) => {
            if !owned_by_import(final_path, token) {
                return Err(format!("Datenbankfehler: {db_error}; Importordner wurde aus Sicherheitsgründen nicht angefasst (Eigentumskennung fehlt)"));
            }
            match fs::rename(final_path, staging) {
                Ok(()) => Err(format!("Datenbankfehler: {db_error}; Importordner zurück ins Staging verschoben")),
                Err(rollback_error) => Err(format!("Datenbankfehler: {db_error}; Rollback fehlgeschlagen: {rollback_error}; Importdateien wurden NICHT gelöscht")),
            }
        }
    }
}

// Read-only recovery inventory. Never delete or rename a folder during startup.
// An ownership marker is only evidence of an interrupted import, not permission
// to remove data. This deliberately errs on the side of retaining files.
#[derive(Default, Debug, PartialEq, Eq)]
struct ImportRecoveryReport {
    staging_candidates: usize,
    published_pending: usize,
    committed_markers: usize,
    unrecognized_markers: usize,
}

fn inspect_import_recovery<F>(library: &Path, project_id_for_slug: F) -> Result<ImportRecoveryReport, String>
where F: Fn(&str) -> Result<Option<String>, String> {
    let mut report=ImportRecoveryReport::default();
    let staging=library.join(".3db").join("staging");
    if staging.exists() {
        for entry in fs::read_dir(&staging).map_err(|e|e.to_string())? {
            let entry=entry.map_err(|e|e.to_string())?;
            let ty=entry.file_type().map_err(|e|e.to_string())?;
            if ty.is_dir() && !ty.is_symlink() {
                match fs::read_to_string(entry.path().join(IMPORT_OWNER_FILE)) {
                    Ok(token) if uuid::Uuid::parse_str(token.trim()).is_ok() => report.staging_candidates+=1,
                    _ => report.unrecognized_markers+=1,
                }
            }
        }
    }
    let projects=library.join("Projects");
    if projects.exists() {
        for entry in fs::read_dir(&projects).map_err(|e|e.to_string())? {
            let entry=entry.map_err(|e|e.to_string())?;
            let ty=entry.file_type().map_err(|e|e.to_string())?;
            if !ty.is_dir() || ty.is_symlink() || !entry.path().join(IMPORT_OWNER_FILE).exists() {continue;}
            let token=fs::read_to_string(entry.path().join(IMPORT_OWNER_FILE)).unwrap_or_default();
            if uuid::Uuid::parse_str(token.trim()).is_err() {report.unrecognized_markers+=1;continue;}
            let slug=entry.file_name().to_string_lossy().into_owned();
            let db_id=project_id_for_slug(&slug)?;
            let manifest_id=fs::read(entry.path().join("project.json")).ok()
                .and_then(|b|serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v|v.get("id").and_then(|x|x.as_str()).map(str::to_owned));
            match (db_id, manifest_id) {
                (Some(a),Some(b)) if a==b => report.committed_markers+=1,
                (None,Some(_)) => report.published_pending+=1,
                _ => report.unrecognized_markers+=1,
            }
        }
    }
    Ok(report)
}

fn check_import_recovery_on_start() {
    let outcome=(||->Result<ImportRecoveryReport,String>{
        let library=setup_root()?;
        let conn=database()?;
        inspect_import_recovery(&library,|slug| {
            use rusqlite::OptionalExtension;
            conn.query_row("SELECT id FROM projects WHERE slug=?1",[slug],|row|row.get(0))
                .optional().map_err(|e|e.to_string())
        })
    })();
    match outcome {
        Ok(r) => {
            // Counts only; do not write identifiable names or filesystem paths to logs.
            let summary=format!("staging={},pending={},committed={},unrecognized={}",
                r.staging_candidates,r.published_pending,r.committed_markers,r.unrecognized_markers);
            diagnostic_write("import_recovery_scan",&summary,0);
        }
        Err(_) => diagnostic_write("import_recovery_scan","ERROR",0),
    }
}

fn import_existing_project_impl(source_dir:String,input:ProjectInput,mappings:Vec<ImportMapping>,expected_entries:Vec<String>)->Result<Project,String>{
    valid_input(&input)?;
    let (source,preview)=import_plan(&source_dir,&mappings)?;
    if !preview.conflicts.is_empty(){return Err("Namenskonflikte vorhanden. Bitte Zuordnung korrigieren, bevor der Import beginnt".into());}
    let actual:Vec<String>=preview.entries.iter().map(|e|format!("{}|{}|{}",e.source_relative_path,e.target_relative_path,e.size_bytes)).collect();
    if actual!=expected_entries{return Err("Der Quellordner oder die Zuordnung hat sich geändert. Bitte erneut analysieren".into());}
    let archive=setup_root()?;
    let projects=archive.join("Projects");
    let base_slug=slugify(&input.name);let mut slug=base_slug.clone();let mut suffix=2;
    while projects.join(&slug).exists(){slug=format!("{base_slug}-{suffix}");suffix+=1;if suffix>10000{return Err("Kein freier Projektname".into());}}
    let now=Utc::now().to_rfc3339();
    let project=Project{id:uuid::Uuid::new_v4().to_string(),name:input.name.trim().into(),slug:slug.clone(),description:input.description,
      status:input.status,tags:input.tags.into_iter().filter(|v|!v.trim().is_empty()).collect(),
      archived:false,created_at:now.clone(),updated_at:now,file_count:preview.entries.len()};
    let staging_root=archive.join(".3db").join("staging");
    fs::create_dir_all(&staging_root).map_err(|e|e.to_string())?;
    let staging=staging_root.join(uuid::Uuid::new_v4().to_string());
    let final_path=projects.join(&slug);
    let token=uuid::Uuid::new_v4().to_string();
    let result=(||->Result<(),String>{
        fs::create_dir(&staging).map_err(|e|e.to_string())?;
        fs::write(staging.join(IMPORT_OWNER_FILE), &token).map_err(|e|e.to_string())?;
        for sub in ["Models","Printfiles","Images","Documents"]{fs::create_dir(staging.join(sub)).map_err(|e|e.to_string())?;}
        for file in &preview.entries {
            let src=source.join(&file.source_relative_path);
            let symlink=fs::symlink_metadata(&src).map_err(|e|e.to_string())?;
            if !symlink.is_file() || symlink.file_type().is_symlink(){return Err("Quellordner wurde beim Import verändert".into());}
            if symlink.len()!=file.size_bytes{return Err(format!("Quelldatei wurde verändert: {}",file.source_relative_path));}
            let dest=staging.join(&file.target_relative_path);
            fs::create_dir_all(dest.parent().ok_or("Ungültiger Zielpfad")?).map_err(|e|e.to_string())?;
            fs::copy(&src,&dest).map_err(|e|format!("Kopieren fehlgeschlagen: {e}"))?;
            if hash_file(&src)?!=hash_file(&dest)? {return Err(format!("Kopie stimmt nicht mit Quelle überein: {}",file.source_relative_path));}
        }
        write_manifest(&staging,&project)?;
        publish_staged_import(&staging, &final_path, &token, || {
            let conn=database()?;
            conn.execute("INSERT INTO projects (id,name,slug,description,status,tags_json,archived,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8)",
              params![project.id,project.name,project.slug,project.description,project.status,
              serde_json::to_string(&project.tags).map_err(|e|e.to_string())?,project.created_at,project.updated_at])
              .map_err(|e|format!("Datenbankeintrag fehlgeschlagen: {e}"))?;
            Ok(())
        })
    })();
    if let Err(e)=result {
        // Never recursively delete a published project, even after a DB error.
        // Only our own private staging directory may be removed.
        if owned_by_import(&staging, &token) {
            if let Err(cleanup_error)=fs::remove_dir_all(&staging) {
                return Err(format!("{e}; Staging-Bereinigung fehlgeschlagen: {cleanup_error}"));
            }
        }
        return Err(e);
    }
    Ok(project)
}
#[tauri::command]
fn list_projects() -> Result<Vec<Project>, String> { all_projects() }
#[tauri::command]
fn get_library_path() -> Result<String, String> { Ok(setup_root()?.display().to_string()) }
fn create_project_impl(input: ProjectInput) -> Result<Project, String> {
    valid_input(&input)?;
    let root = setup_root()?;
    let projects_dir = root.join("Projects");
    let base = slugify(&input.name);
    let mut slug = base.clone();
    let mut n = 2;
    while projects_dir.join(&slug).exists() {
      slug = format!("{}-{}",base,n); n+=1;
      if n>10000 {return Err("Kein freier Projektordner gefunden".into());}
    }
    let path = projects_dir.join(&slug);
    fs::create_dir(&path).map_err(|e|e.to_string())?;
    let now = Utc::now().to_rfc3339();
    let project = Project {id:uuid::Uuid::new_v4().to_string(), name:input.name.trim().to_owned(),slug,
        description:input.description, status:input.status, tags:input.tags.into_iter().map(|x|x.trim().to_owned()).filter(|x|!x.is_empty()).collect(),
        archived:false, created_at:now.clone(), updated_at:now, file_count:0};
    let result = (|| -> Result<(),String> {
      for folder in ["Models","Printfiles","Images","Documents"] {
        fs::create_dir(path.join(folder)).map_err(|e|e.to_string())?;
      }
      write_manifest(&path,&project)?;
      let conn=database()?;
      conn.execute("INSERT INTO projects (id,name,slug,description,status,tags_json,archived,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?8)",
        params![project.id,project.name,project.slug,project.description,project.status,serde_json::to_string(&project.tags).map_err(|e|e.to_string())?,project.created_at,project.updated_at])
        .map_err(|e|e.to_string())?;
      Ok(())
    })();
    if result.is_err() { let _=fs::remove_dir_all(&path); }
    result?;
    Ok(project)
}
#[tauri::command]
fn update_project(id:String, input:ProjectInput, archived:bool) -> Result<(),String> {
    valid_input(&input)?;
    let conn=database()?;
    let existing=all_projects()?.into_iter().find(|p|p.id==id).ok_or("Projekt nicht gefunden")?;
    let path=setup_root()?.join("Projects").join(&existing.slug);
    if !path.is_dir() {return Err("Projektordner fehlt; Änderung abgebrochen".into());}
    let updated=Project {name:input.name.trim().into(),description:input.description,
      status:input.status,tags:input.tags.into_iter().map(|t|t.trim().to_owned()).filter(|t|!t.is_empty()).collect(),
      archived,updated_at:Utc::now().to_rfc3339(),..existing};
    // Save manifest first: failures do not update SQLite. Database failure is reported.
    write_manifest(&path,&updated)?;
    conn.execute("UPDATE projects SET name=?2,description=?3,status=?4,tags_json=?5,archived=?6,updated_at=?7 WHERE id=?1",
      params![updated.id,updated.name,updated.description,updated.status,
        serde_json::to_string(&updated.tags).map_err(|e|e.to_string())?,updated.archived as i64,updated.updated_at])
      .map_err(|e|e.to_string())?;
    Ok(())
}
fn get_project_dir(id:&str)->Result<PathBuf,String>{
    let conn=database()?;
    let slug:String=conn.query_row("SELECT slug FROM projects WHERE id=?1",[id],|r|r.get(0)).map_err(|_|"Projekt nicht gefunden".to_string())?;
    let dir=setup_root()?.join("Projects").join(slug);
    if !dir.is_dir(){return Err("Projektverzeichnis wurde nicht gefunden".into());}
    Ok(dir)
}
fn walk_files(base:&Path)->Result<Vec<ProjectFile>,String>{
  let mut result=Vec::new();
  if !base.exists(){return Ok(result);}
  let mut stack=vec![base.to_path_buf()];
  while let Some(dir)=stack.pop(){
    for entry in fs::read_dir(dir).map_err(|e|e.to_string())? {
      let e=entry.map_err(|e|e.to_string())?;
      let typ=e.file_type().map_err(|e|e.to_string())?;
      if typ.is_symlink(){continue;}
      if typ.is_dir(){stack.push(e.path());continue;}
      if !typ.is_file(){continue;}
      let relative=e.path().strip_prefix(base).map_err(|e|e.to_string())?.to_string_lossy().replace('\\',"/");
      if relative=="project.json"||relative.ends_with(".tmp"){continue;}
      let ext=e.path().extension().map(|s|s.to_string_lossy().to_lowercase()).unwrap_or_default();
      result.push(ProjectFile{relative_path:relative,size_bytes:e.metadata().map_err(|e|e.to_string())?.len(),extension:ext});
    }
  }
  result.sort_by(|a,b|a.relative_path.cmp(&b.relative_path));
  Ok(result)
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct CheckItem { id:String, label:String, rule_type:String, required:bool, completed:bool, matched_files:Vec<String> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct CompletenessSummary { project_id:String, done:i64, total:i64, percent:u8, complete:bool }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct CompletenessResult { summary:CompletenessSummary, items:Vec<CheckItem> }

fn init_checklist(conn:&Connection,id:&str)->Result<(),String>{
    let exists:i64=conn.query_row("SELECT COUNT(*) FROM projects WHERE id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if exists==0 {return Err("Projekt nicht gefunden".into());}
    let n:i64=conn.query_row("SELECT COUNT(*) FROM completeness_rules WHERE project_id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if n==0 {
        // Initialize once, only when the project has never received its checklist.
        // The marker survives deleting all individual rules.
        conn.execute_batch("CREATE TABLE IF NOT EXISTS completeness_initialized (project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE);").map_err(|e|e.to_string())?;
        let initialized:i64=conn.query_row("SELECT COUNT(*) FROM completeness_initialized WHERE project_id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if initialized==0 {
            for (i,(label,kind,required)) in [("STL-Modell vorhanden","stl",true),("3MF-Druckdatei vorhanden","3mf",true),("Projektbild vorhanden","image",true),("PDF-Dokumentation vorhanden","pdf",false),("Testdruck geprüft","task",false)].iter().enumerate(){
                conn.execute("INSERT INTO completeness_rules (id,project_id,label,rule_type,required,position) VALUES (?1,?2,?3,?4,?5,?6)",params![uuid::Uuid::new_v4().to_string(),id,label,kind,if *required {1}else{0},i as i64]).map_err(|e|e.to_string())?;
            }
            conn.execute("INSERT INTO completeness_initialized(project_id) VALUES (?1)",[id]).map_err(|e|e.to_string())?;
        }
    }
    Ok(())
}
#[derive(Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
struct GlobalRequirement { kind:String, required:bool }

fn default_requirements() -> Vec<GlobalRequirement> {
 vec![GlobalRequirement{kind:"stl".into(),required:true},GlobalRequirement{kind:"3mf".into(),required:true},GlobalRequirement{kind:"image".into(),required:true},GlobalRequirement{kind:"pdf".into(),required:false}]
}
fn required_flags(conn:&Connection)->Result<std::collections::HashMap<String,bool>,String>{
 let mut flags=default_requirements().into_iter().map(|r|(r.kind,r.required)).collect::<std::collections::HashMap<_,_>>();
 let mut stmt=conn.prepare("SELECT kind,required FROM global_file_requirements").map_err(|e|e.to_string())?;
 let rows=stmt.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?!=0))).map_err(|e|e.to_string())?;
 for row in rows {let (kind,required)=row.map_err(|e|e.to_string())?;flags.insert(kind,required);}
 Ok(flags)
}
#[tauri::command]
fn get_global_file_requirements()->Result<Vec<GlobalRequirement>,String>{
 let conn=database()?;let flags=required_flags(&conn)?;
 Ok(default_requirements().into_iter().map(|mut x|{x.required=*flags.get(&x.kind).unwrap_or(&x.required);x}).collect())
}
#[tauri::command]
fn set_global_file_requirement(kind:String,required:bool)->Result<Vec<GlobalRequirement>,String>{
 if !["stl","3mf","image","pdf"].contains(&kind.as_str()){return Err("Unbekannter Dateityp".into())}
 let conn=database()?;
 conn.execute("INSERT INTO global_file_requirements(kind,required) VALUES (?1,?2) ON CONFLICT(kind) DO UPDATE SET required=excluded.required",params![kind,if required{1}else{0}]).map_err(|e|e.to_string())?;
 get_global_file_requirements()
}

fn completeness_for(conn:&Connection,id:&str,files:&[ProjectFile])->Result<CompletenessResult,String>{
    init_checklist(conn,id)?;
    let flags=required_flags(conn)?;
    let mut stmt=conn.prepare("SELECT id,label,rule_type,required,done FROM completeness_rules WHERE project_id=?1 ORDER BY position,id").map_err(|e|e.to_string())?;
    let rows=stmt.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?!=0,r.get::<_,i64>(4)?!=0))).map_err(|e|e.to_string())?;
    let mut items=Vec::new();let mut done=0_i64;let mut total=0_i64;
    for row in rows {
        let (rule_id,label,rule_type,stored_required,manually_done)=row.map_err(|e|e.to_string())?;
        let required=flags.get(&rule_type).copied().unwrap_or(stored_required);
        let matches:Vec<String>=files.iter().filter(|f|match rule_type.as_str(){
            "stl"=>f.extension=="stl", "3mf"=>f.extension=="3mf",
            "image"=>["jpg","jpeg","png","webp","bmp","gif"].contains(&f.extension.as_str()),
            "pdf"=>f.extension=="pdf",_=>false
        }).map(|f|f.relative_path.clone()).collect();
        let completed=if rule_type=="task" {manually_done} else {!matches.is_empty()};
        if required {total+=1;if completed {done+=1;}}
        items.push(CheckItem{id:rule_id,label,rule_type,required,completed,matched_files:matches});
    }
    let percent=if total==0 {100}else{((done*100)/total) as u8};
    Ok(CompletenessResult{summary:CompletenessSummary{project_id:id.to_string(),done,total,percent,complete:done==total},items})
}
#[tauri::command]
fn get_project_completeness(id:String)->Result<CompletenessResult,String>{
  let conn=database()?;let files=walk_files(&get_project_dir(&id)?)?;completeness_for(&conn,&id,&files)
}
#[tauri::command]
fn list_completeness_summaries()->Result<Vec<CompletenessSummary>,String>{
  let conn=database()?;let mut stmt=conn.prepare("SELECT id,slug FROM projects").map_err(|e|e.to_string())?;
  let data=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(|e|e.to_string())?;
  let projects:Vec<(String,String)>=data.collect::<Result<_,_>>().map_err(|e|e.to_string())?;
  drop(stmt);
  let base=setup_root()?.join("Projects");let mut res=Vec::new();
  for (id,slug) in projects {let files=walk_files(&base.join(slug))?;res.push(completeness_for(&conn,&id,&files)?.summary);}
  Ok(res)
}
#[tauri::command]
fn set_check_rule(id:String,rule_id:String,required:bool,completed:bool)->Result<CompletenessResult,String>{
  let conn=database()?;let kind:String=conn.query_row("SELECT rule_type FROM completeness_rules WHERE project_id=?1 AND id=?2",params![id,rule_id],|r|r.get(0)).map_err(|_|"Checklisteneintrag nicht gefunden".to_string())?;
  if kind!="task" {return Err("Pflichtdateien werden zentral unter Einstellungen verwaltet".into());}
  conn.execute("UPDATE completeness_rules SET required=?1,done=?2 WHERE project_id=?3 AND id=?4",params![if required{1}else{0},if kind=="task"&&completed{1}else{0},id,rule_id]).map_err(|e|e.to_string())?;
  get_project_completeness(id)
}
#[tauri::command]
fn add_check_task(id:String,label:String)->Result<CompletenessResult,String>{
  let label=label.trim();if label.is_empty()||label.chars().count()>160{return Err("Aufgabe muss zwischen 1 und 160 Zeichen haben".into());}
  let conn=database()?;init_checklist(&conn,&id)?;
  conn.execute("INSERT INTO completeness_rules (id,project_id,label,rule_type,required,position) VALUES (?1,?2,?3,'task',1,COALESCE((SELECT MAX(position)+1 FROM completeness_rules WHERE project_id=?2),0))",params![uuid::Uuid::new_v4().to_string(),id,label]).map_err(|e|e.to_string())?;
  get_project_completeness(id)
}
#[tauri::command]
fn delete_check_task(id:String,rule_id:String)->Result<CompletenessResult,String>{
 let conn=database()?;
 let n=conn.execute("DELETE FROM completeness_rules WHERE id=?1 AND project_id=?2 AND rule_type='task'",params![rule_id,id]).map_err(|e|e.to_string())?;
 if n==0{return Err("Nur selbst angelegte Aufgaben können gelöscht werden".into());}
 get_project_completeness(id)
}
#[tauri::command]
fn list_project_files(id:String)->Result<Vec<ProjectFile>,String>{walk_files(&get_project_dir(&id)?)}
fn hash_file(path: &Path) -> Result<String,String> {
    let file = fs::File::open(path).map_err(|e|e.to_string())?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let count = reader.read(&mut buf).map_err(|e|e.to_string())?;
        if count == 0 { break; }
        hasher.update(&buf[..count]);
    }
    Ok(format!("{:x}",hasher.finalize()))
}
// Preserve observed individual file states independently of full project releases.
// Called during each scan and immediately before a managed replacement.
fn capture_revision(conn:&Connection, id:&str, relative:&str, path:&Path, source:&str)->Result<String,String>{
    let size=fs::metadata(path).map_err(|e|e.to_string())?.len();
    if !fs::metadata(path).map_err(|e|e.to_string())?.is_file() ||
       fs::symlink_metadata(path).map_err(|e|e.to_string())?.file_type().is_symlink() {
        return Err("Nur reguläre Dateien können historisiert werden".into());
    }
    // Fast path: rehash but avoid copying contents on every periodic scan.
    let observed_hash=hash_file(path)?;
    let known:i64=conn.query_row("SELECT EXISTS(SELECT 1 FROM file_revisions WHERE project_id=?1 AND relative_path=?2 AND sha256=?3)",params![id,relative,observed_hash],|r|r.get(0)).map_err(|e|e.to_string())?;
    if known!=0 {return Ok(observed_hash);}
    let root=setup_root()?;
    let hash=persist_object(path,size,&root.join(".3db").join("objects").join("sha256"))?;
    conn.execute("INSERT OR IGNORE INTO snapshot_objects(sha256,size_bytes,created_at) VALUES (?1,?2,?3)",params![hash,size as i64,Utc::now().to_rfc3339()]).map_err(|e|e.to_string())?;
    conn.execute("INSERT OR IGNORE INTO file_revisions(id,project_id,relative_path,sha256,size_bytes,captured_at,source) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![uuid::Uuid::new_v4().to_string(),id,relative,hash,size as i64,Utc::now().to_rfc3339(),source]).map_err(|e|e.to_string())?;
    Ok(hash)
}
fn capture_all_observed(conn:&Connection,id:&str,dir:&Path,files:&std::collections::BTreeMap<String,(String,u64)>)->Result<(),String>{
    for relative in files.keys(){capture_revision(conn,id,relative,&dir.join(relative),"scan")?;}
    Ok(())
}
fn scan_internal(id: &str, mark_baseline: bool) -> Result<ScanResult,String> {
    use std::collections::{BTreeMap,HashMap};
    let dir = get_project_dir(id)?;
    let conn = database()?;
    let files = walk_files(&dir)?;
    let initialized: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM project_scan_state WHERE project_id=?1)",
        [id], |r|r.get::<_,i64>(0)).map_err(|e|e.to_string())? != 0;
    let mut current=BTreeMap::new();
    for file in files {
        // Read actual bytes to distinguish save operations with unchanged content.
        let hash=hash_file(&dir.join(&file.relative_path))?;
        current.insert(file.relative_path,(hash,file.size_bytes));
    }
    // Observe/capture before changing the comparison baseline. A previous state
    // can only be recovered if it was observed earlier or part of a project snapshot.
    capture_all_observed(&conn,id,&dir,&current)?;
    if !initialized || mark_baseline {
        let tx=conn.unchecked_transaction().map_err(|e|e.to_string())?;
        tx.execute("DELETE FROM file_baselines WHERE project_id=?1",[id]).map_err(|e|e.to_string())?;
        for (path,(hash,size)) in &current {
            tx.execute("INSERT INTO file_baselines(project_id,relative_path,sha256,size_bytes) VALUES (?1,?2,?3,?4)",
                params![id,path,hash,*size as i64]).map_err(|e|e.to_string())?;
        }
        tx.execute("INSERT OR REPLACE INTO project_scan_state(project_id,initialized_at) VALUES (?1,?2)",
            params![id,Utc::now().to_rfc3339()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e|e.to_string())?;
        return Ok(ScanResult{changes:vec![],baseline_created:!initialized});
    }
    let mut old=HashMap::new();
    {
        let mut stmt=conn.prepare("SELECT relative_path,sha256,size_bytes FROM file_baselines WHERE project_id=?1").map_err(|e|e.to_string())?;
        let rows=stmt.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))
            .map_err(|e|e.to_string())?;
        for row in rows { let (path,hash,size)=row.map_err(|e|e.to_string())?;old.insert(path,(hash,size)); }
    }
    let mut changes=Vec::new();
    for (path,(hash,size)) in &current {
        let kind=match old.remove(path) {
            None=>Some("added"),
            Some((prior,_)) if prior!=*hash=>Some("modified"),
            _=>None
        };
        if let Some(kind)=kind {changes.push(FileChange{relative_path:path.clone(),kind:kind.into(),size_bytes:*size});}
    }
    for (path,(_,size)) in old {changes.push(FileChange{relative_path:path,kind:"missing".into(),size_bytes:size.max(0) as u64});}
    changes.sort_by(|a,b|a.relative_path.cmp(&b.relative_path));
    Ok(ScanResult{changes,baseline_created:false})
}
fn scan_project_changes_impl(id:String)->Result<ScanResult,String>{scan_internal(&id,false)}
#[tauri::command]
fn acknowledge_project_changes(id:String)->Result<ScanResult,String>{scan_internal(&id,true)}


// A file object is never overwritten. The content hash is the storage key.
fn persist_object(source: &Path, expected_size: u64, store: &Path) -> Result<String, String> {
    fs::create_dir_all(store).map_err(|e|e.to_string())?;
    let staging = store.join(format!(".{}.partial", uuid::Uuid::new_v4()));
    let result = (|| -> Result<String, String> {
        let mut input = BufReader::new(fs::File::open(source).map_err(|e|e.to_string())?);
        let mut output = fs::OpenOptions::new().write(true).create_new(true).open(&staging).map_err(|e|e.to_string())?;
        let mut hasher = Sha256::new();
        let mut length = 0_u64;
        let mut buf = [0_u8; 65536];
        loop {
            let n = input.read(&mut buf).map_err(|e|e.to_string())?;
            if n == 0 { break; }
            output.write_all(&buf[..n]).map_err(|e|e.to_string())?;
            hasher.update(&buf[..n]);
            length += n as u64;
        }
        output.sync_all().map_err(|e|e.to_string())?;
        drop(output);
        if length != expected_size || hash_file(source)? != format!("{:x}",hasher.clone().finalize()) {
            return Err(format!("Datei während Snapshot verändert: {}", source.display()));
        }
        let hash = format!("{:x}",hasher.finalize());
        let bucket = store.join(&hash[..2]);
        fs::create_dir_all(&bucket).map_err(|e|e.to_string())?;
        let dest = bucket.join(&hash);
        if dest.exists() {
            // Do not trust an existing object without validating the bytes.
            if fs::metadata(&dest).map_err(|e|e.to_string())?.len() != length || hash_file(&dest)? != hash {
                return Err(format!("Snapshot-Objekt beschädigt: {hash}"));
            }
        } else {
            // A concurrent writer may race here; never replace an existing object.
            match fs::hard_link(&staging, &dest) {
                Ok(()) => (),
                Err(_e) if dest.exists() => {
                    if hash_file(&dest)? != hash { return Err(format!("Snapshot-Objekt beschädigt: {hash}")); }
                },
                Err(e) => return Err(format!("Objekt konnte nicht angelegt werden: {e}")),
            }
        }
        Ok(hash)
    })();
    let _ = fs::remove_file(&staging);
    result
}

#[tauri::command]
fn list_file_revisions(id:String,relative_path:String)->Result<Vec<FileRevision>,String>{
    get_project_dir(&id)?;
    let conn=database()?;
    let mut stmt=conn.prepare("SELECT id,relative_path,sha256,size_bytes,captured_at,source FROM file_revisions WHERE project_id=?1 AND relative_path=?2 ORDER BY captured_at DESC,id DESC").map_err(|e|e.to_string())?;
    let rows=stmt.query_map(params![id,relative_path],|r|Ok(FileRevision{id:r.get(0)?,relative_path:r.get(1)?,sha256:r.get(2)?,size_bytes:r.get(3)?,captured_at:r.get(4)?,source:r.get(5)?})).map_err(|e|e.to_string())?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
}
fn destination_for(name:&str)->Result<&'static str,String>{
    let ext=Path::new(name).extension().and_then(|e|e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str(){
      "stl"|"step"|"stp"|"obj"|"blend"|"f3d"|"fcstd"|"3ds"=>Ok("Models"),
      "3mf"|"gcode"=>Ok("Printfiles"),
      "png"|"jpg"|"jpeg"|"webp"|"gif"|"bmp"=>Ok("Images"),
      "pdf"|"txt"|"md"|"docx"=>Ok("Documents"),
      _=>Ok("Other"),
    }
}
// Import a list of OS paths from Tauri native drag/drop. File names only are
// used in destination paths; never trust input to supply a relative path.
fn import_project_files_impl(id:String,source_paths:Vec<String>)->Result<FileImportResult,String>{
    if source_paths.is_empty() || source_paths.len()>100 {return Err("Bitte 1 bis 100 Dateien auswählen".into());}
    let base=get_project_dir(&id)?;
    let conn=database()?;
    let mut prepared=Vec::new();
    let mut targets=std::collections::HashSet::new();
    for str_path in &source_paths {
        let source=PathBuf::from(str_path);
        let metadata=fs::symlink_metadata(&source).map_err(|e|e.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink(){return Err("Nur reguläre Dateien ablegen, keine Ordner/Links".into());}
        let name=source.file_name().and_then(|n|n.to_str()).ok_or("Ungültiger Dateiname")?;
        if name=="project.json" || name.ends_with(".tmp") {return Err("Dieser Dateiname ist reserviert".into());}
        let folder=base.join(destination_for(name)?);
        if folder.exists() && fs::symlink_metadata(&folder).map_err(|e|e.to_string())?.file_type().is_symlink(){return Err("Zielordner darf kein symbolischer Link sein".into());}
        let dest=folder.join(name);
        if !targets.insert(dest.to_string_lossy().to_lowercase()){return Err("Mehrere Dateien mit identischem Zielnamen im gleichen Import".into());}
        // Copy to a staged file first. The source can be anywhere, including a working project.
        let staged=base.join(format!(".pv-import-{}.tmp",uuid::Uuid::new_v4()));
        if let Err(e)=fs::copy(&source,&staged){for (_,_,s) in &prepared{let _=fs::remove_file(s);}return Err(e.to_string());}
        if hash_file(&source)?!=hash_file(&staged)? {let _=fs::remove_file(&staged);return Err("Quelldatei wurde während des Imports verändert".into());}
        prepared.push((dest,source,staged));
    }
    let mut res=FileImportResult{imported:vec![],replaced:vec![],unchanged:vec![]};
    for (dest,_src,staged) in &prepared {
        let relative=dest.strip_prefix(&base).map_err(|e|e.to_string())?.to_string_lossy().replace('\\',"/");
        if dest.exists() {
            if fs::symlink_metadata(dest).map_err(|e|e.to_string())?.file_type().is_symlink() || !dest.is_file(){return Err("Ziel ist keine normale Datei".into());}
            let before=capture_revision(&conn,&id,&relative,dest,"before_replace")?;
            if before==hash_file(staged)? {res.unchanged.push(relative);let _=fs::remove_file(staged);continue;}
            let backup=base.join(format!(".pv-replace-{}.tmp",uuid::Uuid::new_v4()));
            fs::rename(dest,&backup).map_err(|e|e.to_string())?;
            if let Err(e)=fs::rename(staged,dest) {let _=fs::rename(&backup,dest);return Err(format!("Ersetzen fehlgeschlagen: {e}"));}
            if let Err(e) = capture_revision(&conn,&id,&relative,dest,"import") {
                // Keep the only known-good working copy if history persistence fails.
                // Do not delete either content in case rollback itself fails.
                let failed_new = base.join(format!(".pv-failed-import-{}.tmp",uuid::Uuid::new_v4()));
                if fs::rename(dest,&failed_new).is_ok() {
                    if fs::rename(&backup,dest).is_ok() {
                        let _=fs::remove_file(&failed_new);
                    } else {
                        return Err(format!("Dateihistorie fehlgeschlagen ({e}); alte Datei liegt sicher unter {} und neue unter {}", backup.display(), failed_new.display()));
                    }
                }
                return Err(format!("Dateihistorie fehlgeschlagen, Änderung abgebrochen: {e}"));
            }
            let _=fs::remove_file(backup);
            res.replaced.push(relative);
        } else {
            fs::create_dir_all(dest.parent().ok_or("Ungültiges Ziel")?).map_err(|e|e.to_string())?;
            fs::rename(staged,dest).map_err(|e|e.to_string())?;
            capture_revision(&conn,&id,&relative,dest,"import")?;
            res.imported.push(relative);
        }
    }
    Ok(res)
}
// Recovery creates a separately named copy, it never changes the active file.
#[tauri::command]
fn recover_file_revision(id:String,revision_id:String)->Result<String,String>{
    let base=get_project_dir(&id)?;
    let conn=database()?;
    let (relative,hash):(String,String)=conn.query_row("SELECT relative_path,sha256 FROM file_revisions WHERE project_id=?1 AND id=?2",params![id,revision_id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"Dateiversion nicht gefunden".to_string())?;
    let rel=Path::new(&relative);
    if rel.is_absolute() || rel.components().any(|c|!matches!(c,std::path::Component::Normal(_))) {return Err("Ungültiger Dateipfad".into());}
    let target_dir=base.join(rel).parent().ok_or("Ungültiger Zielordner")?.to_path_buf();
    fs::create_dir_all(&target_dir).map_err(|e|e.to_string())?;
    if fs::symlink_metadata(&target_dir).map_err(|e|e.to_string())?.file_type().is_symlink(){return Err("Zielordner darf kein symbolischer Link sein".into());}
    let stem=rel.file_stem().and_then(|x|x.to_str()).ok_or("Ungültiger Name")?;
    let extension=rel.extension().and_then(|x|x.to_str()).unwrap_or("");
    let object=setup_root()?.join(".3db").join("objects").join("sha256").join(&hash[..2]).join(&hash);
    if hash_file(&object)?!=hash {return Err("Gesicherte Version ist beschädigt".into());}
    for n in 1..1000 {
      let name=if extension.is_empty(){format!("{stem}-revision-{n}")}else{format!("{stem}-revision-{n}.{extension}")};
      let target=target_dir.join(&name);
      match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
        Ok(mut output)=>{let mut input=fs::File::open(&object).map_err(|e|e.to_string())?;if let Err(e)=std::io::copy(&mut input,&mut output){let _=fs::remove_file(&target);return Err(e.to_string());}output.sync_all().map_err(|e|e.to_string())?;return Ok(target.to_string_lossy().to_string())},
        Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>continue,
        Err(e)=>return Err(e.to_string())
      }
    }
    Err("Zu viele Wiederherstellungskopien vorhanden".into())
}

#[tauri::command]
fn list_project_versions(id: String) -> Result<Vec<VersionInfo>, String> {
    get_project_dir(&id)?;
    let conn = database()?;
    let mut stmt = conn.prepare("SELECT id,project_id,label,description,created_at,file_count,total_size_bytes FROM project_versions WHERE project_id=?1 ORDER BY created_at DESC,id DESC")
        .map_err(|e|e.to_string())?;
    let rows = stmt.query_map([id], |r| Ok(VersionInfo {id:r.get(0)?,project_id:r.get(1)?,label:r.get(2)?,description:r.get(3)?,created_at:r.get(4)?,file_count:r.get(5)?,total_size_bytes:r.get(6)?}))
        .map_err(|e|e.to_string())?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
}

#[tauri::command]
fn list_version_files(version_id: String) -> Result<Vec<VersionFile>, String> {
    let conn=database()?;
    let mut stmt=conn.prepare("SELECT relative_path,size_bytes,sha256 FROM snapshot_files WHERE version_id=?1 ORDER BY relative_path")
        .map_err(|e|e.to_string())?;
    let rows = stmt.query_map([version_id], |r| Ok(VersionFile {relative_path:r.get(0)?,size_bytes:r.get(1)?,sha256:r.get(2)?}))
        .map_err(|e|e.to_string())?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
}

fn create_project_version_impl(id: String, kind: String, description: String) -> Result<VersionResult, String> {
    if kind != "major" && kind != "minor" {return Err("Ungültiger Versionstyp: nur Major oder Minor erlaubt".into());}
    if description.chars().count()>2000 {return Err("Versionsbeschreibung zu lang".into());}
    let project_dir=get_project_dir(&id)?;
    let mut conn=database()?;
    let root=setup_root()?;
    let mut source_files=walk_files(&project_dir)?;
    // Explicitly include project.json, which is deliberately hidden in the working file list.
    if !project_dir.join("project.json").is_file(){return Err("project.json fehlt; Snapshot abgebrochen".into());}
    source_files.push(ProjectFile {relative_path:"project.json".into(),size_bytes:fs::metadata(project_dir.join("project.json")).map_err(|e|e.to_string())?.len(),extension:"json".into()});
    source_files.sort_by(|a,b|a.relative_path.cmp(&b.relative_path));
    let objects_path=root.join(".3db").join("objects").join("sha256");
    let mut version_files=Vec::new();
    for file in &source_files {
        let source=project_dir.join(&file.relative_path);
        // Reject symlinks even if the path changed since the directory walk.
        if fs::symlink_metadata(&source).map_err(|e|e.to_string())?.file_type().is_symlink(){return Err("Verknüpfte Dateien sind nicht als Snapshot zulässig".into());}
        let hash=persist_object(&source,file.size_bytes,&objects_path)?;
        version_files.push(VersionFile {relative_path:file.relative_path.clone(),sha256:hash,size_bytes:file.size_bytes as i64});
    }
    let meta=fs::read_to_string(project_dir.join("project.json")).map_err(|e|e.to_string())?;
    let meta_value:serde_json::Value=serde_json::from_str(&meta).map_err(|e|e.to_string())?;
    let meta_hash=hash_file(&project_dir.join("project.json"))?;
    if version_files.iter().find(|f|f.relative_path=="project.json").map(|f|f.sha256.as_str()) != Some(meta_hash.as_str()) {
        return Err("project.json wurde während der Freigabe verändert; bitte erneut versuchen".into());
    }
    let total_size_bytes=version_files.iter().map(|f|f.size_bytes).sum();
    let info=VersionInfo{id:uuid::Uuid::new_v4().to_string(),project_id:id.clone(),label:String::new(),description,created_at:Utc::now().to_rfc3339(),file_count:version_files.len() as i64,total_size_bytes};
    let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
    // Determine the highest valid existing vMAJOR.MINOR, ignoring older free-form labels.
    // The decision and INSERT share one write transaction, preventing duplicate version numbers.
    let (mut major,mut minor)=(0u64,0u64);
    let mut has_valid=false;
    {
        let mut stmt=tx.prepare("SELECT label FROM project_versions WHERE project_id=?1").map_err(|e|e.to_string())?;
        let rows=stmt.query_map([&id],|r|r.get::<_,String>(0)).map_err(|e|e.to_string())?;
        for item in rows {
            let text=item.map_err(|e|e.to_string())?;
            if let Some(raw)=text.strip_prefix('v') {
                if let Some((a,b))=raw.split_once('.') {
                    if a.chars().all(|c|c.is_ascii_digit()) && !a.is_empty() && b.chars().all(|c|c.is_ascii_digit()) && !b.is_empty() {
                        if let (Ok(ma),Ok(mi))=(a.parse::<u64>(),b.parse::<u64>()) {
                            if !has_valid || (ma,mi)>(major,minor) {(major,minor)=(ma,mi);has_valid=true;}
                        }
                    }
                }
            }
        }
    }
    let (next_major,next_minor)=if !has_valid {(1u64,0u64)} else if kind=="major" {
        (major.checked_add(1).ok_or("Major-Version zu groß")?,0)
    } else {(major,minor.checked_add(1).ok_or("Minor-Version zu groß")?)};
    let generated_label=format!("v{}.{}",next_major,next_minor);
    let mut info=info;
    info.label=generated_label;
    tx.execute("INSERT INTO project_versions(id,project_id,label,description,created_at,metadata_json,file_count,total_size_bytes) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![info.id,info.project_id,info.label,info.description,info.created_at,meta_value.to_string(),info.file_count,info.total_size_bytes]).map_err(|e|e.to_string())?;
    for file in &version_files {
        tx.execute("INSERT OR IGNORE INTO snapshot_objects(sha256,size_bytes,created_at) VALUES (?1,?2,?3)",params![file.sha256,file.size_bytes,info.created_at]).map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO snapshot_files(version_id,relative_path,sha256,size_bytes) VALUES (?1,?2,?3,?4)",params![info.id,file.relative_path,file.sha256,file.size_bytes]).map_err(|e|e.to_string())?;
    }
    tx.commit().map_err(|e|e.to_string())?;
    Ok(VersionResult{version:info,files:version_files})
}

#[tauri::command]
fn open_project_folder(app:tauri::AppHandle,id:String)->Result<(),String>{
    let path=get_project_dir(&id)?;
    app.opener().open_path(path.to_string_lossy().to_string(),None::<&str>).map_err(|e|e.to_string())
}
#[tauri::command]
fn open_project_file(app:tauri::AppHandle,id:String,relative_path:String)->Result<(),String>{
    let base=get_project_dir(&id)?.canonicalize().map_err(|e|e.to_string())?;
    let candidate=base.join(relative_path).canonicalize().map_err(|e|e.to_string())?;
    if !candidate.starts_with(&base)||!candidate.is_file(){return Err("Datei liegt nicht innerhalb des Projektordners".into());}
    app.opener().open_path(candidate.to_string_lossy().to_string(),None::<&str>).map_err(|e|e.to_string())
}
// Variante und Datei-Zuordnung sind Datenbank-Metadaten; Dateien bleiben im gemeinsamen Projektordner.
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct Variant { id:String, project_id:String, name:String, description:String, created_at:String }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct VariantFileLink { variant_id:String, relative_path:String }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct VariantOverview { variants:Vec<Variant>, links:Vec<VariantFileLink> }
fn variant_overview(conn:&Connection,id:&str)->Result<VariantOverview,String>{
  let mut query=conn.prepare("SELECT id,project_id,name,description,created_at FROM variants WHERE project_id=?1 ORDER BY created_at,id")
    .map_err(|e|e.to_string())?;
  let variants=query.query_map([id],|r|Ok(Variant{id:r.get(0)?,project_id:r.get(1)?,name:r.get(2)?,description:r.get(3)?,created_at:r.get(4)?}))
    .map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
  let mut query=conn.prepare("SELECT variant_id,relative_path FROM variant_files WHERE project_id=?1 ORDER BY relative_path,variant_id")
    .map_err(|e|e.to_string())?;
  let links=query.query_map([id],|r|Ok(VariantFileLink{variant_id:r.get(0)?,relative_path:r.get(1)?}))
    .map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
  Ok(VariantOverview{variants,links})
}
fn sync_variant_manifest(conn:&Connection,id:&str)->Result<(),String>{
  let dir=get_project_dir(id)?;
  let overview=variant_overview(conn,id)?;
  let path=dir.join("project.json");
  let mut doc:serde_json::Value=serde_json::from_slice(&fs::read(&path).map_err(|e|e.to_string())?)
    .map_err(|e|e.to_string())?;
  doc["variants"]=serde_json::to_value(&overview.variants).map_err(|e|e.to_string())?;
  doc["variantFiles"]=serde_json::to_value(&overview.links).map_err(|e|e.to_string())?;
  let tmp=dir.join("project.json.variants.tmp");
  fs::write(&tmp,serde_json::to_vec_pretty(&doc).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
  replace_manifest_safely(&tmp,&path)
}
#[tauri::command]
fn list_project_variants(id:String)->Result<VariantOverview,String>{
  get_project_dir(&id)?;
  variant_overview(&database()?,&id)
}
#[tauri::command]
fn create_variant(id:String,name:String,description:String)->Result<VariantOverview,String>{
  get_project_dir(&id)?;
  let name=name.trim();
  if name.is_empty()||name.chars().count()>80{return Err("Variantenname muss 1 bis 80 Zeichen lang sein".into());}
  if description.chars().count()>1000{return Err("Variantenbeschreibung zu lang".into());}
  let conn=database()?;
  conn.execute("INSERT INTO variants(id,project_id,name,description,created_at) VALUES (?1,?2,?3,?4,?5)",
    params![uuid::Uuid::new_v4().to_string(),id,name,description.trim(),Utc::now().to_rfc3339()])
    .map_err(|e|format!("Variante konnte nicht angelegt werden (Name bereits vorhanden?): {e}"))?;
  sync_variant_manifest(&conn,&id)?;
  variant_overview(&conn,&id)
}
#[tauri::command]
fn update_variant(id:String,variant_id:String,name:String,description:String)->Result<VariantOverview,String>{
  get_project_dir(&id)?;
  let name=name.trim();
  if name.is_empty()||name.chars().count()>80{return Err("Variantenname muss 1 bis 80 Zeichen lang sein".into());}
  if description.chars().count()>1000{return Err("Variantenbeschreibung zu lang".into());}
  let conn=database()?;
  let count=conn.execute("UPDATE variants SET name=?3,description=?4 WHERE project_id=?1 AND id=?2",
    params![id,variant_id,name,description.trim()]).map_err(|e|e.to_string())?;
  if count==0{return Err("Variante nicht gefunden".into());}
  sync_variant_manifest(&conn,&id)?;
  variant_overview(&conn,&id)
}
#[tauri::command]
fn delete_variant(id:String,variant_id:String)->Result<VariantOverview,String>{
  get_project_dir(&id)?;
  let conn=database()?;
  let count=conn.execute("DELETE FROM variants WHERE project_id=?1 AND id=?2",params![id,variant_id]).map_err(|e|e.to_string())?;
  if count==0{return Err("Variante nicht gefunden".into());}
  sync_variant_manifest(&conn,&id)?;
  variant_overview(&conn,&id)
}
#[tauri::command]
fn set_file_variants(id:String,relative_path:String,variant_ids:Vec<String>)->Result<VariantOverview,String>{
  let base=get_project_dir(&id)?.canonicalize().map_err(|e|e.to_string())?;
  if relative_path.is_empty()||relative_path.contains('\\')||relative_path.split('/').any(|s|s==".."||s=="."||s.is_empty()){
    return Err("Ungültiger relativer Dateipfad".into());
  }
  let file=base.join(&relative_path).canonicalize().map_err(|_|"Projektdatei nicht gefunden".to_string())?;
  if !file.starts_with(&base)||!file.is_file()||relative_path=="project.json"{
    return Err("Die Zuordnung ist nur für vorhandene Projektdateien möglich".into());
  }
  let mut conn=database()?;
  let tx=conn.transaction().map_err(|e|e.to_string())?;
  tx.execute("DELETE FROM variant_files WHERE project_id=?1 AND relative_path=?2",params![id,relative_path]).map_err(|e|e.to_string())?;
  let mut uniq=std::collections::HashSet::new();
  for variant_id in variant_ids {
    if !uniq.insert(variant_id.clone()){continue;}
    let found:i64=tx.query_row("SELECT COUNT(*) FROM variants WHERE project_id=?1 AND id=?2",params![id,variant_id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if found!=1{return Err("Variante gehört nicht zum Projekt".into());}
    tx.execute("INSERT INTO variant_files(project_id,variant_id,relative_path) VALUES (?1,?2,?3)",params![id,variant_id,relative_path]).map_err(|e|e.to_string())?;
  }
  tx.commit().map_err(|e|e.to_string())?;
  sync_variant_manifest(&conn,&id)?;
  variant_overview(&conn,&id)
}


// Separate optional metadata: existing projects from 0.7.1 require no schema changes to `projects`.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all="camelCase")]
struct ProjectDetails {
    category: String,
    author: String,
    license: String,
    source_url: String,
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
struct ThreeMfInfo {
    relative_path: String,
    fields: std::collections::BTreeMap<String,String>,
    sources: Vec<String>,
    warnings: Vec<String>,
}
#[tauri::command]
fn get_project_details(id:String) -> Result<ProjectDetails,String> {
    let conn=database()?;
    let details=conn.query_row("SELECT category,author,license,source_url FROM project_details WHERE project_id=?1",[&id], |r| Ok(ProjectDetails{category:r.get(0)?,author:r.get(1)?,license:r.get(2)?,source_url:r.get(3)?}));
    match details {
        Ok(v)=>Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows)=>{
            let found:i64=conn.query_row("SELECT count(*) FROM projects WHERE id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if found==0 {Err("Projekt nicht gefunden".into())} else {Ok(ProjectDetails::default())}
        },
        Err(e)=>Err(e.to_string()),
    }
}
fn save_project_details_impl(id:String, details:ProjectDetails)->Result<ProjectDetails,String>{
    if [details.category.as_str(),details.author.as_str(),details.license.as_str()].iter().any(|x|x.chars().count()>120)
        || details.source_url.chars().count()>1000 {return Err("Metadatenfeld ist zu lang".into());}
    if !details.source_url.trim().is_empty()
        && !(details.source_url.starts_with("https://")||details.source_url.starts_with("http://")) {
        return Err("Quelladresse muss mit https:// oder http:// beginnen".into());
    }
    let dir=get_project_dir(&id)?;
    let conn=database()?;
    // Preserve unrelated data in project.json (especially variants).
    let manifest_path=dir.join("project.json");
    let mut manifest:serde_json::Value=serde_json::from_slice(&fs::read(&manifest_path).map_err(|e|e.to_string())?)
        .map_err(|e|format!("project.json konnte nicht gelesen werden: {e}"))?;
    manifest["details"]=serde_json::to_value(&details).map_err(|e|e.to_string())?;
    let tmp=dir.join("project.json.metadata.tmp");
    fs::write(&tmp,serde_json::to_vec_pretty(&manifest).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    // DB is primary. If manifest update fails, report explicitly so user knows synchronization is needed.
    conn.execute("INSERT INTO project_details(project_id,category,author,license,source_url,updated_at) VALUES (?1,?2,?3,?4,?5,?6)
       ON CONFLICT(project_id) DO UPDATE SET category=excluded.category,author=excluded.author,license=excluded.license,source_url=excluded.source_url,updated_at=excluded.updated_at",
       params![id,details.category.trim(),details.author.trim(),details.license.trim(),details.source_url.trim(),Utc::now().to_rfc3339()]).map_err(|e|e.to_string())?;
    let _=fs::remove_file(&manifest_path);
    fs::rename(&tmp,&manifest_path).map_err(|e|format!("Metadaten in SQLite gespeichert, aber project.json konnte nicht aktualisiert werden: {e}"))?;
    Ok(details)
}
fn search_json_settings(value:&serde_json::Value,out:&mut std::collections::BTreeMap<String,String>,depth:usize){
    if depth>5{return;}
    const KEYS:&[(&str,&str)]=&[
      ("printer_model","Druckermodell"),("printer_settings_id","Druckerprofil"),
      ("filament_type","Filamenttyp"),("filament_settings_id","Filamentprofil"),
      ("nozzle_diameter","Düsendurchmesser"),("layer_height","Schichthöhe"),
      ("initial_layer_print_height","Erste Schichthöhe"),
      ("print_settings_id","Druckprofil"),
    ];
    match value {
      serde_json::Value::Object(obj)=>for (k,v) in obj {
        if let Some((_,label))=KEYS.iter().find(|(key,_)|k.as_str()==*key) {
            let text=match v{serde_json::Value::String(x)=>x.clone(),serde_json::Value::Number(x)=>x.to_string(),serde_json::Value::Array(arr)=>arr.iter().take(8).filter_map(|x|x.as_str().map(str::to_owned).or_else(||x.as_f64().map(|n|n.to_string()))).collect::<Vec<_>>().join(", "),_=>String::new()};
            if !text.is_empty()&&text.len()<500 {out.entry((*label).into()).or_insert(text);}
        }
        search_json_settings(v,out,depth+1);
      },
      serde_json::Value::Array(arr)=>for v in arr.iter().take(20){search_json_settings(v,out,depth+1)},
      _=>(),
    }
}
#[tauri::command]
fn inspect_3mf(id:String,relative_path:String)->Result<ThreeMfInfo,String>{
    // Resolve only files indexed inside the project folder; traversal and symlinks forbidden.
    let base=get_project_dir(&id)?.canonicalize().map_err(|e|e.to_string())?;
    let path=base.join(&relative_path).canonicalize().map_err(|e|e.to_string())?;
    if !path.starts_with(&base)||!path.is_file(){return Err("Datei liegt nicht innerhalb des Projektordners".into());}
    if path.extension().and_then(|e|e.to_str()).map(|e|!e.eq_ignore_ascii_case("3mf")).unwrap_or(true){return Err("Nur 3MF-Dateien können analysiert werden".into());}
    let input=fs::File::open(&path).map_err(|e|e.to_string())?;
    let mut archive=zip::ZipArchive::new(input).map_err(|e|format!("Ungültiger 3MF-ZIP-Container: {e}"))?;
    let mut fields=std::collections::BTreeMap::new();let mut sources=Vec::new();let mut warnings=Vec::new();
    let limit=archive.len().min(1500);
    for i in 0..limit {
        let entry=archive.by_index(i).map_err(|e|e.to_string())?;
        let name=entry.name().replace('\\',"/");
        let lower=name.to_ascii_lowercase();
        if !lower.starts_with("metadata/") || !lower.ends_with(".config") || !lower.contains("project_settings") {continue;}
        if entry.size()>2_000_000 {warnings.push(format!("{name}: Metadaten zu groß"));continue;}
        let mut buf=Vec::new();entry.take(2_000_001).read_to_end(&mut buf).map_err(|e|e.to_string())?;
        if buf.len()>2_000_000 {warnings.push(format!("{name}: Größenlimit überschritten"));continue;}
        if let Ok(settings)=serde_json::from_slice::<serde_json::Value>(&buf){search_json_settings(&settings,&mut fields,0);sources.push(name)}
        else {warnings.push(format!("{name}: Kein unterstütztes JSON-Format"));}
    }
    if fields.is_empty(){warnings.push("Keine unterstützten Bambu-Studio-Druckparameter gefunden. Nicht jede 3MF enthält Slicer-Metadaten.".into());}
    Ok(ThreeMfInfo{relative_path,fields,sources,warnings})
}


// Privacy-conscious release audit diagnostics. No project names, file paths,
// filenames, tags, or raw command arguments are written to this log.
// A maximum of 2 MiB is retained; the previous log is rotated to .old.
fn diagnostic_dir() -> Result<PathBuf,String> {
    let base = dirs::data_local_dir().or_else(dirs::home_dir)
        .ok_or("Kein lokales Benutzerverzeichnis gefunden")?;
    let dir=base.join("3db").join("logs");
    fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
    Ok(dir)
}
fn diagnostic_write(event:&str, outcome:&str, elapsed_ms:u128) {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::{Mutex,OnceLock};
    static LOG_LOCK:OnceLock<Mutex<()>>=OnceLock::new();
    let Ok(_guard)=LOG_LOCK.get_or_init(||Mutex::new(())).lock() else{return};
    let Ok(dir)=diagnostic_dir() else{return};
    let path=dir.join("3db-audit.log");
    if fs::metadata(&path).map(|m|m.len()>2_000_000).unwrap_or(false) {
       let old=dir.join("3db-audit.log.old");
       let _=fs::remove_file(&old);
       if fs::rename(&path,&old).is_err(){return;}
    }
    if let Ok(mut file)=OpenOptions::new().create(true).append(true).open(path) {
      let _=writeln!(file,"{} | {} | {} | {}ms",Utc::now().to_rfc3339(),event,outcome,elapsed_ms);
    }
}
fn audit_call<T>(event:&str, op:impl FnOnce()->Result<T,String>)->Result<T,String>{
    let start=std::time::Instant::now();
    diagnostic_write(event,"START",0);
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(op));
    match result {
      Ok(result)=>{
        diagnostic_write(event,if result.is_ok(){"OK"}else{"ERROR"},start.elapsed().as_millis());
        result
      }
      Err(panic)=>{
        diagnostic_write(event,"PANIC",start.elapsed().as_millis());
        std::panic::resume_unwind(panic)
      }
    }
}
#[tauri::command]
fn get_diagnostic_log_path()->Result<String,String>{
    Ok(diagnostic_dir()?.join("3db-audit.log").to_string_lossy().into_owned())
}
#[tauri::command]
fn open_diagnostic_folder(app:tauri::AppHandle)->Result<(),String>{
    use tauri_plugin_opener::OpenerExt;
    let dir=diagnostic_dir()?;
    app.opener().open_path(dir.to_string_lossy().to_string(),None::<&str>).map_err(|e|e.to_string())
}
#[tauri::command]
fn import_existing_project(source_dir:String,input:ProjectInput,mappings:Vec<ImportMapping>,expected_entries:Vec<String>)->Result<Project,String>{
    audit_call("project_import",||import_existing_project_impl(source_dir,input,mappings,expected_entries))
}
#[tauri::command]
fn create_project(input:ProjectInput)->Result<Project,String>{
    audit_call("project_create",||create_project_impl(input))
}
#[tauri::command]
fn import_project_files(id:String,source_paths:Vec<String>)->Result<FileImportResult,String>{
    audit_call("file_import",||import_project_files_impl(id,source_paths))
}
#[tauri::command]
fn create_project_version(id:String,kind:String,description:String)->Result<VersionResult,String>{
    audit_call("snapshot_create",||create_project_version_impl(id,kind,description))
}
#[tauri::command]
fn scan_project_changes(id:String)->Result<ScanResult,String>{
    audit_call("file_scan",||scan_project_changes_impl(id))
}
#[tauri::command]
fn save_project_details(id:String,details:ProjectDetails)->Result<ProjectDetails,String>{
    audit_call("project_metadata_save",||save_project_details_impl(id,details))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(){
  diagnostic_write("app_start",env!("CARGO_PKG_VERSION"),0);
  check_import_recovery_on_start();
  tauri::Builder::default()
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![list_projects,get_library_path,create_project,update_project,list_project_files,open_project_folder,open_project_file,scan_project_changes,acknowledge_project_changes,list_project_versions,list_version_files,create_project_version,list_file_revisions,import_project_files,recover_file_revision,preview_project_import,import_existing_project,list_project_variants,create_variant,update_variant,delete_variant,set_file_variants,get_project_details,save_project_details,inspect_3mf,get_project_completeness,list_completeness_summaries,set_check_rule,add_check_task,delete_check_task,get_global_file_requirements,set_global_file_requirement,get_diagnostic_log_path,open_diagnostic_folder])
    .run(tauri::generate_context!())
    .expect("3db konnte nicht gestartet werden");
}

#[cfg(test)]
mod import_safety_tests {
    use super::*;

    fn sandbox() -> PathBuf {
        let p=std::env::temp_dir().join(format!("3db-import-audit-{}",uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap(); p
    }
    fn prepare(base:&Path) -> (PathBuf,PathBuf,String) {
        let staging=base.join("staging"); let published=base.join("project");
        let token=uuid::Uuid::new_v4().to_string();
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join(IMPORT_OWNER_FILE),&token).unwrap();
        fs::write(staging.join("model.stl"),b"test-bytes").unwrap();
        (staging,published,token)
    }
    #[test]
    fn recovery_detects_staged_import_without_modifying_it() {
        let base=sandbox(); let dir=base.join(".3db").join("staging").join("pending");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(IMPORT_OWNER_FILE),uuid::Uuid::new_v4().to_string()).unwrap();
        fs::write(dir.join("model.stl"),b"valuable").unwrap();
        let r=inspect_import_recovery(&base,|_|Ok(None)).unwrap();
        assert_eq!(r.staging_candidates,1);
        assert_eq!(fs::read(dir.join("model.stl")).unwrap(),b"valuable");
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn recovery_detects_published_import_without_database_entry() {
        let base=sandbox(); let dir=base.join("Projects").join("model");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(IMPORT_OWNER_FILE),uuid::Uuid::new_v4().to_string()).unwrap();
        fs::write(dir.join("project.json"),r#"{"id":"id-001"}"#).unwrap();
        let r=inspect_import_recovery(&base,|_|Ok(None)).unwrap();
        assert_eq!(r.published_pending,1);
        assert!(dir.exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn recovery_recognizes_committed_import_with_leftover_marker() {
        let base=sandbox(); let dir=base.join("Projects").join("model");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(IMPORT_OWNER_FILE),uuid::Uuid::new_v4().to_string()).unwrap();
        fs::write(dir.join("project.json"),r#"{"id":"id-001"}"#).unwrap();
        let r=inspect_import_recovery(&base,|slug|Ok((slug=="model").then(||"id-001".into()))).unwrap();
        assert_eq!(r.committed_markers,1);
        assert!(dir.join(IMPORT_OWNER_FILE).exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn recovery_flags_mismatched_ids_and_unknown_ownership() {
        let base=sandbox(); let dir=base.join("Projects").join("model");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(IMPORT_OWNER_FILE),uuid::Uuid::new_v4().to_string()).unwrap();
        fs::write(dir.join("project.json"),r#"{"id":"other-id"}"#).unwrap();
        let r=inspect_import_recovery(&base,|_|Ok(Some("id-001".into()))).unwrap();
        assert_eq!(r.unrecognized_markers,1);
        assert!(dir.exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn recovery_ignores_regular_projects_without_markers() {
        let base=sandbox(); let dir=base.join("Projects").join("normal");
        fs::create_dir_all(&dir).unwrap();
        let r=inspect_import_recovery(&base,|_|Ok(None)).unwrap();
        assert_eq!(r,ImportRecoveryReport::default());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn import_success_publishes_without_marker() {
        let base=sandbox(); let (staging,published,token)=prepare(&base);
        assert!(publish_staged_import(&staging,&published,&token,||Ok(())).is_ok());
        assert_eq!(fs::read(published.join("model.stl")).unwrap(),b"test-bytes");
        assert!(!published.join(IMPORT_OWNER_FILE).exists());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn import_database_error_rolls_back_to_staging() {
        let base=sandbox(); let (staging,published,token)=prepare(&base);
        let result=publish_staged_import(&staging,&published,&token,||Err("injected SQLite failure".into()));
        assert!(result.is_err()); assert!(staging.join("model.stl").exists());
        assert!(!published.exists()); fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn import_existing_destination_is_never_overwritten() {
        let base=sandbox(); let (staging,published,token)=prepare(&base);
        fs::create_dir(&published).unwrap();
        fs::write(published.join("keep.txt"),b"do not touch").unwrap();
        assert!(publish_staged_import(&staging,&published,&token,||Ok(())).is_err());
        assert_eq!(fs::read(published.join("keep.txt")).unwrap(),b"do not touch");
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn import_wrong_owner_is_rejected() {
        let base=sandbox(); let (staging,published,_token)=prepare(&base);
        assert!(publish_staged_import(&staging,&published,"not-ours",||Ok(())).is_err());
        assert!(staging.join("model.stl").exists());
        fs::remove_dir_all(base).unwrap();
    }
}


#[cfg(test)]
mod migration_safety_tests {
    use super::*;

    fn historical_snapshot(conn: &Connection) {
        // Minimal historical snapshot tables; newer schema objects are created by the real initializer.
        conn.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, description TEXT NOT NULL DEFAULT '', status TEXT NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]', archived INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
        CREATE TABLE snapshot_objects (sha256 TEXT PRIMARY KEY, size_bytes INTEGER NOT NULL, created_at TEXT NOT NULL);
        CREATE TABLE project_versions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, label TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, metadata_json TEXT NOT NULL, file_count INTEGER NOT NULL, total_size_bytes INTEGER NOT NULL, UNIQUE(project_id,label));
        CREATE TABLE snapshot_files (version_id TEXT NOT NULL, relative_path TEXT NOT NULL, sha256 TEXT NOT NULL, size_bytes INTEGER NOT NULL, PRIMARY KEY(version_id,relative_path));
        INSERT INTO projects VALUES('p1','Example','example','','idea','[]',0,'2024','2024');
        INSERT INTO snapshot_objects VALUES('abc',12,'2024');
        INSERT INTO project_versions VALUES('v1','p1','v1.0','','2024','{}',1,12);
        INSERT INTO snapshot_files VALUES('v1','Models/test.stl','abc',12);") .unwrap();
    }

    #[test]
    fn legacy_snapshot_backfill_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        historical_snapshot(&conn);
        initialize_database_schema(&mut conn).unwrap();
        initialize_database_schema(&mut conn).unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM file_revisions WHERE project_id='p1' AND relative_path='Models/test.stl'",[],|r|r.get(0)).unwrap();
        assert_eq!(count, 1);
        let count: i64 = conn.query_row("SELECT count(*) FROM projects WHERE id='p1'",[],|r|r.get(0)).unwrap();
        assert_eq!(count, 1);
        let integrity: String = conn.query_row("PRAGMA integrity_check",[],|r|r.get(0)).unwrap();
        assert_eq!(integrity,"ok");
    }

    #[test]
    fn failed_backfill_rolls_back_schema_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        // Conflicting legacy view intentionally makes the backfill fail after the CREATE TABLE statements.
        conn.execute_batch("CREATE VIEW snapshot_files AS SELECT 1 AS broken;").unwrap();
        let before: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE name='file_revisions'",[],|r|r.get(0)).unwrap();
        assert_eq!(before,0);
        assert!(initialize_database_schema(&mut conn).is_err());
        let after: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE name='file_revisions'",[],|r|r.get(0)).unwrap();
        assert_eq!(after,0,"partial schema changes must roll back on migration failure");
        let integrity: String = conn.query_row("PRAGMA integrity_check",[],|r|r.get(0)).unwrap();
        assert_eq!(integrity,"ok");
    }

    #[test]
    fn empty_database_initializes_twice() {
        let mut conn = Connection::open_in_memory().unwrap();
        initialize_database_schema(&mut conn).unwrap();
        initialize_database_schema(&mut conn).unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='projects'",[],|r|r.get(0)).unwrap();
        assert_eq!(count,1);
    }
}


#[cfg(test)]
mod audit10_isolation_tests {
    use super::*;
    fn sandbox() -> PathBuf {
        let path=std::env::temp_dir().join(format!("3db-audit10-test-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(path.join(".3db")).unwrap();
        fs::create_dir_all(path.join("Projects")).unwrap();
        fs::write(path.join(".3db").join("3db.db"),b"test").unwrap();
        path
    }
    #[test]
    fn rejects_unmarked_and_invalid_markers() {
        let base=sandbox(); let production=base.join("production");
        assert!(validate_audit_copy(&base,&production).is_err());
        fs::write(base.join(AUDIT_COPY_MARKER),"not our marker").unwrap();
        assert!(validate_audit_copy(&base,&production).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn rejects_production_overlap() {
        let base=sandbox(); let prod=base.join("production");
        fs::create_dir_all(&prod).unwrap();
        fs::write(base.join(AUDIT_COPY_MARKER),AUDIT_COPY_MARKER_VALUE).unwrap();
        assert!(validate_audit_copy(&base,&prod).is_err());
        assert!(validate_audit_copy(&prod,&prod).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn accepts_a_marked_independent_copy() {
        let base=sandbox(); let prod=std::env::temp_dir().join(format!("3db-prod-imaginary-{}",uuid::Uuid::new_v4()));
        fs::write(base.join(AUDIT_COPY_MARKER),AUDIT_COPY_MARKER_VALUE).unwrap();
        assert_eq!(validate_audit_copy(&base,&prod).unwrap(),base.canonicalize().unwrap());
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    #[ignore = "Only run through tools/audit10_windows.ps1 with an isolated restored copy"]
    fn audit10_real_copy_migration() {
        assert_eq!(std::env::var("THREEDB_AUDIT_CONFIRM").unwrap(),"ISOLATED_COPY_ONLY");
        let path=root().expect("Audit-Ordner nicht freigegeben");
        let db_path=path.join(".3db").join("3db.db");
        let before=Connection::open_with_flags(&db_path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let count_before:i64=before.query_row("SELECT COUNT(*) FROM projects",[],|r|r.get(0)).unwrap();
        let snapshots_before:i64=before.query_row("SELECT COUNT(*) FROM project_versions",[],|r|r.get(0)).unwrap();
        drop(before);
        let conn=database().expect("Rust-Migration auf isolierter Kopie gescheitert");
        let integrity:String=conn.query_row("PRAGMA integrity_check",[],|r|r.get(0)).unwrap();
        assert_eq!(integrity,"ok");
        let foreign_errors:i64=conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|r|r.get(0)).unwrap();
        assert_eq!(foreign_errors,0);
        let count_after:i64=conn.query_row("SELECT COUNT(*) FROM projects",[],|r|r.get(0)).unwrap();
        let snapshots_after:i64=conn.query_row("SELECT COUNT(*) FROM project_versions",[],|r|r.get(0)).unwrap();
        assert_eq!(count_before,count_after,"Projektanzahl wurde verändert");
        assert_eq!(snapshots_before,snapshots_after,"Snapshotanzahl wurde verändert");
        println!("AUDIT10 PASS: Rust-Migration auf Kopie: projects={count_after}, snapshots={snapshots_after}, integrity=ok, foreign_key_errors=0");
    }
}
