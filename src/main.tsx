import React, {useEffect, useMemo, useRef, useState} from 'react';
import {createRoot} from 'react-dom/client';
import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {getCurrentWebview} from '@tauri-apps/api/webview';
import {FolderOpen, LayoutDashboard, Library, Settings, Plus, Search, File, ExternalLink, Archive, ArrowLeft, Moon, Sun, Tag, X, RotateCw, GitBranch, Save, ChevronDown, ChevronUp, Upload, History, Copy, FolderInput} from 'lucide-react';
import './style.css';

type Status='idea'|'working'|'testing'|'finished';
type Project={id:string;name:string;slug:string;description:string;status:Status;tags:string[];createdAt:string;updatedAt:string;archived:boolean;fileCount:number};
type ProjectFile={relativePath:string;extension:string;sizeBytes:number};
type FileChange={relativePath:string;kind:'added'|'modified'|'missing';sizeBytes:number};
type ScanResult={changes:FileChange[];baselineCreated:boolean};
type VersionInfo={id:string;projectId:string;label:string;description:string;createdAt:string;fileCount:number;totalSizeBytes:number};
type VersionFile={relativePath:string;sha256:string;sizeBytes:number};
type VersionResult={version:VersionInfo;files:VersionFile[]};
type FileRevision={id:string;relativePath:string;sha256:string;sizeBytes:number;capturedAt:string;source:string};
type FileImportResult={imported:string[];replaced:string[];unchanged:string[]};
type ImportEntry={sourceRelativePath:string;targetRelativePath:string;sizeBytes:number;category:string};
type ImportPreview={suggestedName:string;entries:ImportEntry[];totalSizeBytes:number;conflicts:string[];skippedLinks:number};
type Variant={id:string;projectId:string;name:string;description:string;createdAt:string};
type VariantFileLink={variantId:string;relativePath:string};
type VariantOverview={variants:Variant[];links:VariantFileLink[]};
type ImportMapping={sourceRelativePath:string;category:string;targetName:string|null};
type Input={name:string;description:string;status:Status;tags:string[]};
type ProjectDetails={category:string;author:string;license:string;sourceUrl:string};
type ThreeMfInfo={relativePath:string;fields:Record<string,string>;sources:string[];warnings:string[]};
type CheckItem={id:string;label:string;ruleType:'stl'|'3mf'|'image'|'pdf'|'task';required:boolean;completed:boolean;matchedFiles:string[]};
type CheckSummary={projectId:string;done:number;total:number;percent:number;complete:boolean};
type CheckResult={summary:CheckSummary;items:CheckItem[]};
type Requirement={kind:string;required:boolean};
type PanelId='changes'|'versions'|'completeness'|'metadata'|'variants'|'threeMf'|'files'|'info';
type PanelConfig={id:PanelId;visible:boolean;column:1|2};
const panelNames:Record<PanelId,string>={changes:'Dateiänderungen',versions:'Versionsverwaltung',completeness:'Vollständigkeit',metadata:'Projektmetadaten',variants:'Projektvarianten',threeMf:'3MF-Druckinformationen',files:'Projektdateien',info:'Projektinformationen'};
const defaultPanels:PanelConfig[]=(Object.keys(panelNames) as PanelId[]).map(id=>({id,visible:true,column:(['metadata','variants','threeMf','info'].includes(id)?2:1) as 1|2}));
const blankDetails:ProjectDetails={category:'',author:'',license:'',sourceUrl:''};
type Page='dashboard'|'projects'|'settings'|'import';
type VersionBump='major'|'minor';
// Alte, freie Versionsbezeichnungen bleiben lesbar, sind aber keine gültigen SemVer-Nummern.
function nextVersion(versions:VersionInfo[],kind:VersionBump):string {
 const valid=versions.map(v=>/^v(\d+)\.(\d+)$/.exec(v.label)).filter((m):m is RegExpExecArray=>m!==null).map(m=>[Number(m[1]),Number(m[2])]).filter(([major,minor])=>Number.isSafeInteger(major)&&Number.isSafeInteger(minor));
 if(!valid.length)return 'v1.0';
 valid.sort((a,b)=>b[0]-a[0]||b[1]-a[1]);
 const [major,minor]=valid[0];
 return kind==='major'?`v${major+1}.0`:`v${major}.${minor+1}`;
}
const statusNames:Record<Status,string>={idea:'In Planung',working:'In Arbeit',testing:'Testphase',finished:'Fertig'};
const defaultInput:Input={name:'',description:'',status:'idea',tags:[]};
// Preserve commas and spaces while editing; normalize only when saving.
const parseTags=(text:string):string[]=>[...new Set(text.split(',').map(tag=>tag.trim()).filter(Boolean))];
const date=(str:string)=>new Date(str).toLocaleDateString('de-DE');
const bytes=(n:number)=>n>1048576?(n/1048576).toFixed(1)+' MB':n>1024?(n/1024).toFixed(1)+' KB':n+' B';
function App(){
 const [projects,setProjects]=useState<Project[]>([]),[page,setPage]=useState<Page>('dashboard'),[chosen,setChosen]=useState<string|null>(null);
 const [search,setSearch]=useState(''),[filter,setFilter]=useState<string>('all'),[dashboardFilter,setDashboardFilter]=useState<string>('all'),[includeArchived,setIncludeArchived]=useState(false);
 const [modal,setModal]=useState<'new'|'edit'|null>(null),[form,setForm]=useState<Input>(defaultInput),[formTagsText,setFormTagsText]=useState(''),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
 const [changes,setChanges]=useState<FileChange[]>([]),[scanBusy,setScanBusy]=useState(false),[scanInitial,setScanInitial]=useState(false);
 const [versions,setVersions]=useState<VersionInfo[]>([]),[versionKind,setVersionKind]=useState<VersionBump>('minor'),[versionDescription,setVersionDescription]=useState(''),[versionBusy,setVersionBusy]=useState(false),[expandedVersion,setExpandedVersion]=useState<string|null>(null),[snapshotFiles,setSnapshotFiles]=useState<VersionFile[]>([]);
 const [fileHistory,setFileHistory]=useState<Record<string,FileRevision[]>>({}),[dropBusy,setDropBusy]=useState(false),[dragOver,setDragOver]=useState(false);
 const [importSource,setImportSource]=useState(''),[importPreview,setImportPreview]=useState<ImportPreview|null>(null),[importMappings,setImportMappings]=useState<ImportMapping[]>([]);
 const [importData,setImportData]=useState<Input>(defaultInput),[importTagsText,setImportTagsText]=useState(''),[importBusy,setImportBusy]=useState(false);
 const [variantInfo,setVariantInfo]=useState<VariantOverview>({variants:[],links:[]}),[variantName,setVariantName]=useState(''),[variantDesc,setVariantDesc]=useState(''),[variantBusy,setVariantBusy]=useState(false),[editVariant,setEditVariant]=useState<string|null>(null);
 const [files,setFiles]=useState<ProjectFile[]>([]),[libraryPath,setLibraryPath]=useState(''),[showPreviews,setShowPreviews]=useState(()=>localStorage.getItem('pv.preview')!=='false');
 const [theme,setTheme]=useState<'system'|'dark'|'light'>(()=>(localStorage.getItem('pv.theme') as 'system'|'dark'|'light')||'system');
 const [systemDark,setSystemDark]=useState(window.matchMedia('(prefers-color-scheme: dark)').matches);
 const dark=theme==='system'?systemDark:theme==='dark';
 useEffect(()=>{if(projects.length)void refreshCheckSummaries().catch(e=>setError(String(e)));else setCheckSummaries({})},[projects]);
 useEffect(()=>{const media=window.matchMedia('(prefers-color-scheme: dark)');const fn=(e:MediaQueryListEvent)=>setSystemDark(e.matches);media.addEventListener('change',fn);return()=>media.removeEventListener('change',fn)},[]);
 useEffect(()=>{localStorage.setItem('pv.preview',String(showPreviews))},[showPreviews]);
 useEffect(()=>{localStorage.setItem('pv.theme',theme)},[theme]);
 const load=async()=>{try{setProjects(await invoke<Project[]>('list_projects'));setLibraryPath(await invoke<string>('get_library_path'));setError('')}catch(e){setError(String(e))}};
 useEffect(()=>{void load()},[]);
 useEffect(()=>{let active=true;Promise.all(projects.map(async p=>[p.id,(await invoke<ProjectDetails>('get_project_details',{id:p.id})).category] as const)).then(v=>{if(active)setCategories(Object.fromEntries(v))}).catch(()=>{});return()=>{active=false}},[projects]);
 const current=projects.find(p=>p.id===chosen)||null;
 const nextLabel=useMemo(()=>nextVersion(versions,versionKind),[versions,versionKind]);
 const [details,setDetails]=useState<ProjectDetails>(blankDetails),[detailsOpen,setDetailsOpen]=useState(false),[detailsBusy,setDetailsBusy]=useState(false);
 const [mfResults,setMfResults]=useState<Record<string,ThreeMfInfo>>({});
 const [mfErrors,setMfErrors]=useState<Record<string,string>>({});
 const [mfLoading,setMfLoading]=useState<Set<string>>(()=>new Set());
 const [mfSelected,setMfSelected]=useState<string|null>(null),[mfOpen,setMfOpen]=useState(false);
 const mfFiles=useMemo(()=>files.filter(f=>f.extension.toLowerCase()==='3mf'),[files]);
 const mfFileKey=mfFiles.map(f=>`${f.relativePath}:${f.sizeBytes}`).join('|');
 const mfInfo=mfSelected?mfResults[mfSelected]:undefined;
 // Automatische Analyse ausschließlich beim Öffnen, nicht bei zugeklapptem Panel.
 // Maximal drei gleichzeitige ZIP-Analysen; Ergebnisse bleiben pro Datei im Speicher.
 useEffect(()=>{
   if(!chosen||!mfOpen)return;
   let cancelled=false;
   setMfResults({});setMfErrors({});setMfLoading(new Set(mfFiles.map(f=>f.relativePath)));
   setMfSelected(mfFiles[0]?.relativePath??null);
   const paths=mfFiles.map(f=>f.relativePath);
   let index=0;
   const worker=async()=>{
     while(!cancelled){
       const n=index++;
       if(n>=paths.length)return;
       const path=paths[n];
       try{
         const info=await invoke<ThreeMfInfo>('inspect_3mf',{id:chosen,relativePath:path});
         if(!cancelled)setMfResults(old=>({...old,[path]:info}));
       }catch(e){if(!cancelled)setMfErrors(old=>({...old,[path]:String(e)}));}
       finally{if(!cancelled)setMfLoading(old=>{const next=new Set(old);next.delete(path);return next})}
     }
   };
   for(let i=0;i<Math.min(paths.length,3);i++)void worker();
   return()=>{cancelled=true};
 },[chosen,mfOpen,mfFileKey]);
 const [globalRequirements,setGlobalRequirements]=useState<Requirement[]>([]);
 const [requirementsBusy,setRequirementsBusy]=useState(false);
 const [layoutColumns,setLayoutColumns]=useState<1|2>(()=>localStorage.getItem('3db.layout.columns')==='1'?1:2);
 const [panels,setPanels]=useState<PanelConfig[]>(()=>{try{const raw=JSON.parse(localStorage.getItem('3db.layout.panels')||'null');if(!Array.isArray(raw))return defaultPanels;const entries=raw.filter((r:any)=>defaultPanels.some(p=>p.id===r.id)).map((r:any)=>({id:r.id as PanelId,visible:r.visible!==false,column:(r.column===2?2:1) as 1|2}));const oldDefault=entries.length===8&&entries.every((p:any,i:number)=>p.id===defaultPanels[i].id&&p.visible&&p.column===(i===7?2:1));return oldDefault?defaultPanels:[...entries,...defaultPanels.filter(p=>!entries.some((x:any)=>x.id===p.id))]}catch{return defaultPanels}});
 const pointerDrag=useRef<{id:PanelId;pointerId:number;startX:number;startY:number;active:boolean;target?:PanelId;column?:1|2}|null>(null);
 const [dragPanel,setDragPanel]=useState<PanelId|null>(null);
 useEffect(()=>{localStorage.setItem('3db.layout.columns',String(layoutColumns))},[layoutColumns]);
 useEffect(()=>{localStorage.setItem('3db.layout.panels',JSON.stringify(panels))},[panels]);
 useEffect(()=>{void invoke<Requirement[]>('get_global_file_requirements').then(setGlobalRequirements).catch(e=>setError(String(e)))},[]);
 const changeRequirement=async(kind:string,required:boolean)=>{setRequirementsBusy(true);try{const v=await invoke<Requirement[]>('set_global_file_requirement',{kind,required});setGlobalRequirements(v);await refreshCheckSummaries();if(chosen)await refreshChecklist(chosen)}catch(e){setError(String(e))}finally{setRequirementsBusy(false)}};
 const editPanel=(id:PanelId,change:Partial<PanelConfig>)=>setPanels(old=>old.map(p=>p.id===id?{...p,...change}:p));
 const movePanel=(id:PanelId,direction:-1|1)=>setPanels(old=>{const item=old.find(p=>p.id===id);if(!item)return old;const ids=old.filter(p=>layoutColumns===1||p.column===item.column);const i=ids.findIndex(p=>p.id===id);const target=ids[i+direction];if(!target)return old;const copy=[...old],a=copy.findIndex(p=>p.id===id),b=copy.findIndex(p=>p.id===target.id);[copy[a],copy[b]]=[copy[b],copy[a]];return copy});
 const [dragTarget,setDragTarget]=useState<string|null>(null);
 const placePanel=(moving:PanelId, column:1|2, before?:PanelId)=>{
  setPanels(old=>{
   const entry=old.find(p=>p.id===moving);if(!entry)return old;
   const rest=old.filter(p=>p.id!==moving);
   // Keep the destination column contiguous in the saved panel order.
   const targetIndex=before?rest.findIndex(p=>p.id===before):-1;
   let insertAt=targetIndex;
   if(insertAt<0){const last=rest.map((p,i)=>p.column===column?i:-1).filter(i=>i>=0).pop();insertAt=last===undefined?rest.length:last+1}
   rest.splice(insertAt,0,{...entry,column});
   return rest;
  });setDragPanel(null);setDragTarget(null);
 };
 // Pointer-based panel drag is deliberately separate from Tauri's native OS file drag/drop.
 // HTML5 draggable is unreliable inside WebView2 when native OS file drops are enabled.
 const beginLayoutDrag=(e:React.PointerEvent<HTMLButtonElement>,id:PanelId)=>{
  if(e.button!==0)return;
  e.preventDefault();e.currentTarget.setPointerCapture(e.pointerId);
  pointerDrag.current={id,pointerId:e.pointerId,startX:e.clientX,startY:e.clientY,active:false};
 };
 const updateLayoutDrag=(e:React.PointerEvent<HTMLButtonElement>)=>{
  const drag=pointerDrag.current;if(!drag||drag.pointerId!==e.pointerId)return;
  if(!drag.active&&Math.hypot(e.clientX-drag.startX,e.clientY-drag.startY)<5)return;
  if(!drag.active){drag.active=true;setDragPanel(drag.id)}
  // elementFromPoint works even while the grip owns pointer capture.
  const elem=document.elementFromPoint(e.clientX,e.clientY) as HTMLElement|null;
  const item=elem?.closest<HTMLElement>('[data-layout-item]');
  const col=elem?.closest<HTMLElement>('[data-layout-column]');
  const target=item?.dataset.layoutItem as PanelId|undefined;
  const column=Number(col?.dataset.layoutColumn) as 1|2;
  drag.target=target&&target!==drag.id?target:undefined;
  drag.column=(column===1||column===2)?column:undefined;
  setDragTarget(drag.target|| (drag.column?'column'+drag.column:null));
 };
 const finishLayoutDrag=(e:React.PointerEvent<HTMLButtonElement>)=>{
  const drag=pointerDrag.current;if(!drag||drag.pointerId!==e.pointerId)return;
  if(drag.active&&drag.column){placePanel(drag.id,drag.column,drag.target)}
  pointerDrag.current=null;setDragPanel(null);setDragTarget(null);
 };
 const cancelLayoutDrag=()=>{pointerDrag.current=null;setDragPanel(null);setDragTarget(null)};
 const [checkOpen,setCheckOpen]=useState(false),[checkInfo,setCheckInfo]=useState<CheckResult|null>(null),[checkTask,setCheckTask]=useState(''),[checkBusy,setCheckBusy]=useState(false);
 const [checkSummaries,setCheckSummaries]=useState<Record<string,CheckSummary>>({}),[completenessFilter,setCompletenessFilter]=useState('all');
 const refreshCheckSummaries=async()=>{
   const summaries=await invoke<CheckSummary[]>('list_completeness_summaries');
   setCheckSummaries(Object.fromEntries(summaries.map(x=>[x.projectId,x])));
 };
 const refreshChecklist=async(id:string)=>{
   const value=await invoke<CheckResult>('get_project_completeness',{id});setCheckInfo(value);
   setCheckSummaries(s=>({...s,[id]:value.summary}));
 };
 const editCheck=async(rule:CheckItem,field:'required'|'completed',value:boolean)=>{
   if(!chosen)return;setCheckBusy(true);setError('');
   try{const next=await invoke<CheckResult>('set_check_rule',{id:chosen,ruleId:rule.id,required:field==='required'?value:rule.required,completed:field==='completed'?value:rule.completed});setCheckInfo(next);setCheckSummaries(s=>({...s,[chosen]:next.summary}));}
   catch(e){setError(String(e))}finally{setCheckBusy(false)}
 };
 const addTask=async()=>{
   if(!chosen||!checkTask.trim())return;setCheckBusy(true);setError('');
   try{const next=await invoke<CheckResult>('add_check_task',{id:chosen,label:checkTask.trim()});setCheckInfo(next);setCheckSummaries(s=>({...s,[chosen]:next.summary}));setCheckTask('')}
   catch(e){setError(String(e))}finally{setCheckBusy(false)}
 };
 const deleteTask=async(ruleId:string)=>{
   if(!chosen||!window.confirm('Diese Aufgabe aus der Checkliste entfernen?'))return;setCheckBusy(true);
   try{const next=await invoke<CheckResult>('delete_check_task',{id:chosen,ruleId});setCheckInfo(next);setCheckSummaries(s=>({...s,[chosen]:next.summary}));}
   catch(e){setError(String(e))}finally{setCheckBusy(false)}
 };
 const [categoryFilter,setCategoryFilter]=useState('all'),[categories,setCategories]=useState<Record<string,string>>({});
 const [fileAreaOpen,setFileAreaOpen]=useState(false),[fileHistoriesOpen,setFileHistoriesOpen]=useState<Set<string>>(()=>new Set());
 useEffect(()=>{
   if(!chosen){setDetails(blankDetails);setDetailsOpen(false);setMfOpen(false);setChanges([]);setVersions([]);setVariantInfo({variants:[],links:[]});setExpandedVersion(null);setFileHistory({});return;}
   setFileAreaOpen(false);setFileHistoriesOpen(new Set());setDetailsOpen(false);setMfOpen(false);setMfSelected(null);setCheckOpen(false);setCheckInfo(null);setCheckTask('');
   let cancelled=false;let running=false;
   const scan=async()=>{if(running)return;running=true;try{
      const [result,list]=await Promise.all([invoke<ScanResult>('scan_project_changes',{id:chosen}),invoke<ProjectFile[]>('list_project_files',{id:chosen})]);
      if(!cancelled){setChanges(result.changes);setFiles(list);void refreshChecklist(chosen).catch(()=>{});if(result.baselineCreated)setScanInitial(true);void refreshHistory(chosen,list.map(f=>f.relativePath)).catch(e=>{if(!cancelled)setError(String(e))})}
   }catch(e){if(!cancelled)setError(String(e))}finally{running=false}};
   void scan();void invoke<ProjectDetails>('get_project_details',{id:chosen}).then(v=>{if(!cancelled)setDetails(v)}).catch(e=>{if(!cancelled)setError(String(e))});void invoke<VariantOverview>('list_project_variants',{id:chosen}).then(v=>{if(!cancelled)setVariantInfo(v)}).catch(e=>{if(!cancelled)setError(String(e))});void invoke<VersionInfo[]>('list_project_versions',{id:chosen}).then(v=>{if(!cancelled)setVersions(v)}).catch(e=>{if(!cancelled)setError(String(e))});const timer=window.setInterval(()=>void scan(),15000);
   return()=>{cancelled=true;window.clearInterval(timer)};
 },[chosen]);
 const refreshHistory=async(id:string, paths:string[])=>{
  const entries=await Promise.all(paths.map(async path=>[path,await invoke<FileRevision[]>('list_file_revisions',{id,relativePath:path})] as const));
  setFileHistory(Object.fromEntries(entries));
 };
 const importDropped=async(id:string,paths:string[])=>{
  if(!paths.length)return;
  if(!window.confirm(`${paths.length} Datei(en) in das Projekt kopieren?\n\nBei gleichem Dateinamen wird die vorhandene Datei zuerst historisiert und anschließend ersetzt. Die externen Originaldateien bleiben unberührt.`))return;
  setDropBusy(true);setError('');
  try{
   const result=await invoke<FileImportResult>('import_project_files',{id,sourcePaths:paths});
   setNotice(`Import: ${result.imported.length} neu, ${result.replaced.length} ersetzt (alte Stände gesichert), ${result.unchanged.length} unverändert.`);
   const next=await invoke<ProjectFile[]>('list_project_files',{id});setFiles(next);
   await refreshHistory(id,next.map(x=>x.relativePath));
   const changes=await invoke<ScanResult>('scan_project_changes',{id});setChanges(changes.changes);
   await load();await refreshChecklist(id);
  }catch(e){setError(String(e))}finally{setDropBusy(false);setDragOver(false)}
 };
 // Tauri's native OS drop event returns real Windows paths, not browser File objects.
 useEffect(()=>{
  if(!chosen)return;
  let disposed=false;let stop:(()=>void)|undefined;
  getCurrentWebview().onDragDropEvent(e=>{
   if(disposed)return;
   if(e.payload.type==='enter'||e.payload.type==='over')setDragOver(true);
   if(e.payload.type==='leave')setDragOver(false);
   if(e.payload.type==='drop') {setDragOver(false);void importDropped(chosen,e.payload.paths);}
  }).then(unlisten=>{if(disposed)unlisten();else stop=unlisten}).catch(e=>setError(String(e)));
  return()=>{disposed=true;stop?.();setDragOver(false)};
 },[chosen]);
 const refreshVariants=async(id:string)=>setVariantInfo(await invoke<VariantOverview>('list_project_variants',{id}));
 const saveVariant=async()=>{
  if(!chosen||!variantName.trim())return;
  setVariantBusy(true);setError('');
  try{
   const params={id:chosen,name:variantName.trim(),description:variantDesc.trim()};
   const result=editVariant?await invoke<VariantOverview>('update_variant',{...params,variantId:editVariant}):await invoke<VariantOverview>('create_variant',params);
   setVariantInfo(result);setVariantName('');setVariantDesc('');setEditVariant(null);
   setNotice(editVariant?'Variante aktualisiert.':'Variante angelegt. Gemeinsame Projektdateien bleiben erhalten.');
  }catch(e){setError(String(e))}finally{setVariantBusy(false)}
 };
 const removeVariant=async(id:string,variantId:string,name:string)=>{
  if(!window.confirm(`Variante „${name}“ löschen? Die Projektdateien und Dateihistorien bleiben erhalten.`))return;
  try{setVariantInfo(await invoke<VariantOverview>('delete_variant',{id,variantId}));setNotice('Variante gelöscht, keine Projektdateien gelöscht.')}catch(e){setError(String(e))}
 };
 const assignVariant=async(id:string,path:string,variantId:string,checked:boolean)=>{
  const ids=variantInfo.links.filter(x=>x.relativePath===path).map(x=>x.variantId);
  const next=checked?[...ids,variantId]:ids.filter(v=>v!==variantId);
  setVariantBusy(true);setError('');
  try{setVariantInfo(await invoke<VariantOverview>('set_file_variants',{id,relativePath:path,variantIds:next}))}
  catch(e){setError(String(e));await refreshVariants(id).catch(()=>{})}
  finally{setVariantBusy(false)}
 };
 const restoreRevision=async(id:string,revisionId:string)=>{
  if(!window.confirm('Diesen Dateistand als zusätzliche Kopie im Projektverzeichnis ablegen? Die aktuelle Datei bleibt unverändert.'))return;
  try{
   const path=await invoke<string>('recover_file_revision',{id,revisionId});
   setNotice(`Wiederherstellungskopie erstellt: ${path}`);
   setFiles(await invoke<ProjectFile[]>('list_project_files',{id}));
  }catch(e){setError(String(e))}
 };
 const scanNow=async()=>{if(!chosen)return;setScanBusy(true);try{const result=await invoke<ScanResult>('scan_project_changes',{id:chosen});setChanges(result.changes);setFiles(await invoke<ProjectFile[]>('list_project_files',{id:chosen}));await refreshHistory(chosen,(await invoke<ProjectFile[]>('list_project_files',{id:chosen})).map(f=>f.relativePath))}catch(e){setError(String(e))}finally{setScanBusy(false)}};
 const acknowledge=async()=>{if(!chosen)return;setScanBusy(true);try{await invoke('acknowledge_project_changes',{id:chosen});setChanges([]);setNotice('Aktueller Dateistand als Vergleichsbasis übernommen. Dies ist noch keine Versionsfreigabe.')}catch(e){setError(String(e))}finally{setScanBusy(false)}};
 const releaseVersion=async()=>{
   if(!chosen)return;
   setVersionBusy(true);setError('');
   try{
     const result=await invoke<VersionResult>('create_project_version',{id:chosen,kind:versionKind,description:versionDescription.trim()});
     const next=await invoke<VersionInfo[]>('list_project_versions',{id:chosen});
     setVersions(next);setExpandedVersion(null);setSnapshotFiles([]);
     setVersionDescription('');
     setNotice('Version '+result.version.label+' mit '+result.files.length+' Dateien als unveränderlicher Snapshot gespeichert. Der Änderungsmonitor bleibt davon unabhängig.');
   }catch(e){setError(String(e))}finally{setVersionBusy(false)}
 };
 const toggleVersion=async(id:string)=>{
   if(expandedVersion===id){setExpandedVersion(null);return}
   try{const v=await invoke<VersionFile[]>('list_version_files',{versionId:id});setSnapshotFiles(v);setExpandedVersion(id)}catch(e){setError(String(e))}
 };
 const visible=useMemo(()=>projects.filter(p=>(includeArchived||!p.archived)&&(filter==='all'||p.status===filter)&&(categoryFilter==='all'||(categories[p.id]||'')===categoryFilter)&&(completenessFilter==='all'||(checkSummaries[p.id]&&(completenessFilter==='complete'?checkSummaries[p.id].complete:!checkSummaries[p.id].complete)))&&[p.name,p.description,categories[p.id]||'',...p.tags].join(' ').toLowerCase().includes(search.trim().toLowerCase())),[projects,includeArchived,filter,categoryFilter,categories,search,completenessFilter,checkSummaries]);
 const active=projects.filter(p=>!p.archived);
 const dashboard=active.filter(p=>dashboardFilter==='all'||p.status===dashboardFilter);
 const counts={all:active.length,working:active.filter(p=>p.status==='working').length,finished:active.filter(p=>p.status==='finished').length,idea:active.filter(p=>p.status==='idea').length};
 const pickImportFolder=async()=>{
   try {
    const chosenDir=await open({directory:true,multiple:false,title:'Bestehenden Projektordner auswählen'});
    if(typeof chosenDir !== 'string')return;
    setImportSource(chosenDir);setImportMappings([]);setImportPreview(null);setError('');
    setImportBusy(true);
    const info=await invoke<ImportPreview>('preview_project_import',{sourceDir:chosenDir,mappings:[]});
    setImportPreview(info);setImportData({...defaultInput,name:info.suggestedName});setImportTagsText('');
   }catch(e){setError(String(e))}finally{setImportBusy(false)}
 };
 const updateImportMapping=async(path:string,category:string,targetName:string|null)=>{
   if(!importSource)return;
   const previous=importMappings.filter(m=>m.sourceRelativePath!==path);
   const next=[...previous,{sourceRelativePath:path,category,targetName}];
   setImportMappings(next);setImportBusy(true);
   try{setImportPreview(await invoke<ImportPreview>('preview_project_import',{sourceDir:importSource,mappings:next}));setError('')}
   catch(e){setError(String(e))}finally{setImportBusy(false)}
 };
 const commitImport=async()=>{
   if(!importPreview||importPreview.conflicts.length||!importData.name.trim()||importBusy)return;
   if(!window.confirm(`${importPreview.entries.length} Dateien aus dem ausgewählten Ordner als neues Projekt „${importData.name}“ kopieren? Der ursprüngliche Ordner bleibt unverändert.`))return;
   setImportBusy(true);setError('');
   try{
     const created=await invoke<Project>('import_existing_project',{sourceDir:importSource,input:{...importData,tags:parseTags(importTagsText)},mappings:importMappings,expectedEntries:importPreview.entries.map(e=>`${e.sourceRelativePath}|${e.targetRelativePath}|${e.sizeBytes}`)});
     await load();setChosen(created.id);setPage('projects');setImportPreview(null);setImportMappings([]);
     setNotice(`${created.fileCount} Dateien als Projekt „${created.name}“ importiert. Originale unverändert.`);
   }catch(e){setError(String(e))}finally{setImportBusy(false)}
 };
 const action=async(name:string,args:Record<string, unknown>,ok?:string)=>{try{setBusy(true);setError('');await invoke(name,args);await load();if(ok)setNotice(ok)}catch(e){setError(String(e))}finally{setBusy(false)}};
 const startEdit=(p:Project)=>{setForm({name:p.name,description:p.description,status:p.status,tags:p.tags});setFormTagsText(p.tags.join(', '));setModal('edit')};
 const submit=async(e:React.FormEvent)=>{e.preventDefault();if(!form.name.trim())return;const input={...form,tags:parseTags(formTagsText)};setBusy(true);setError('');try{
    if(modal==='new'){const p=await invoke<Project>('create_project',{input});await load();setChosen(p.id);setPage('projects');setNotice('Projekt und Ordner wurden angelegt.');}
    else if(current){await invoke('update_project',{id:current.id,input,archived:current.archived});await load();setNotice('Projekt gespeichert.');}
    setModal(null);
   }catch(e){setError(String(e))}finally{setBusy(false)} };
 const saveDetails=async()=>{
  if(!current)return;setDetailsBusy(true);setError('');
  try{const saved=await invoke<ProjectDetails>('save_project_details',{id:current.id,details});setDetails(saved);setNotice('Projektmetadaten gespeichert.');setCategories(old=>({...old,[current.id]:saved.category}));}
  catch(e){setError(String(e))}finally{setDetailsBusy(false)}
 };
 const card=(p:Project)=><button className={'project-card '+(!showPreviews?'compact':'')} key={p.id} onClick={()=>{setChosen(p.id);setPage('projects');setNotice('')}}>
   {showPreviews&&<div className="cover"><span className="cover-symbol">{p.status==='finished'?'◈':p.status==='working'?'⬡':'◇'}</span><span className="cover-type">{p.fileCount} Dateien</span></div>}
   <div className="card-content"><div className="card-title">{p.name}</div>{!showPreviews&&<p className="description">{p.description||'Keine Beschreibung'}</p>}
   <div className="card-bottom">{checkSummaries[p.id]&&<span className={"tag completeness-tag "+(checkSummaries[p.id].complete?"complete":"incomplete")}>{checkSummaries[p.id].percent}% vollständig</span>}{categories[p.id]&&<span className="tag">{categories[p.id]}</span>}<span className={'status '+p.status}>{statusNames[p.status]}</span><span className="muted">{date(p.updatedAt)}</span></div>
   <div className="tags">{p.tags.slice(0,3).map(t=><span className="tag" key={t}>{t}</span>)}{!showPreviews&&<span className="muted">{p.fileCount} Dateien</span>}</div></div></button>;
 return <div className={'app '+(dark?'dark':'light')}>
   <aside className="sidebar"><div className="logo"><span>⬡</span> 3db</div>
    <button className={'nav '+(page==='dashboard'&&!chosen?'active':'')} onClick={()=>{setChosen(null);setPage('dashboard')}}><LayoutDashboard size={18}/> Dashboard</button>
    <button className={'nav '+(page==='projects'?'active':'')} onClick={()=>{setChosen(null);setPage('projects')}}><Library size={18}/> Projekte</button>
    <button className={'nav '+(page==='import'?'active':'')} onClick={()=>{setChosen(null);setPage('import')}}><FolderInput size={18}/> Projekt importieren</button>
    <button className={'nav bottom '+(page==='settings'?'active':'')} onClick={()=>{setChosen(null);setPage('settings')}}><Settings size={18}/> Einstellungen</button>
    <span className="version">3db 1.0.0 · First Release</span>
   </aside>
   <main className="main">
    {error&&<div className="alert error"><span>{error}</span><button onClick={()=>setError('')} aria-label="Fehlermeldung schließen"><X size={16}/></button></div>}
    {notice&&<div className="alert notice"><span>{notice}</span><button onClick={()=>setNotice('')} aria-label="Hinweis schließen"><X size={16}/></button></div>}
    {current? <>
      <div className="top"><div><button className="back" onClick={()=>setChosen(null)}><ArrowLeft size={16}/> Zurück zu Projekten</button><h1>{current.name}</h1><p>{current.description||'Noch keine Beschreibung vorhanden.'}</p></div><div className="buttons"><button className="btn" onClick={()=>startEdit(current)}>Bearbeiten</button><button className="btn primary" disabled={busy} onClick={()=>void action('open_project_folder',{id:current.id})}><FolderOpen size={16}/> Projektordner öffnen</button></div></div>
      {(()=>{
       const blocks:Record<PanelId,React.ReactNode>={
        changes: (<section className="panel change-panel"><div className="panel-heading"><div><h2>Dateiänderungen {changes.length>0&&<span className="change-counter">{changes.length}</span>}</h2><p>Automatischer Abgleich alle 15 Sekunden mit dem zuletzt bestätigten Dateistand. Unabhängig von freigegebenen Snapshots.</p></div><div className="buttons"><button className="btn" disabled={scanBusy} onClick={()=>void scanNow()}><RotateCw size={15}/> Prüfen</button><button className="btn" disabled={scanBusy||changes.length===0} onClick={()=>void acknowledge()}>Änderungen als geprüft markieren</button></div></div>{changes.length?changes.map(c=><div className="file-row" key={c.relativePath}><div className="file-meta"><File size={16}/><div><strong>{c.relativePath}</strong><small>{c.kind==='added'?'Neu hinzugefügt':c.kind==='modified'?'Inhalt verändert':'Datei fehlt'}</small></div></div><span className={'file-change '+c.kind}>{c.kind==='added'?'Neu':c.kind==='modified'?'Geändert':'Fehlt'}</span></div>):<div className="muted">Keine unbestätigten Dateiänderungen.{scanInitial?' Der erste Scan hat die Vergleichsbasis angelegt.':''}</div>}</section>),
        versions: (<section className="panel snapshot-panel">
       <div className="panel-heading"><div><h2><GitBranch size={18}/> Versionsverwaltung</h2><p>Freigegebene Stände enthalten sämtliche Projektdateien inklusive project.json. Änderungen als geprüft markieren ist eine getrennte Funktion.</p></div></div>
       <div className="version-create"><label>Änderungstyp<select value={versionKind} onChange={e=>setVersionKind(e.target.value as VersionBump)}><option value="minor">Minor (+0.1)</option><option value="major">Major (+1.0)</option></select></label><label>Nächste Versionsnummer<input value={nextLabel} readOnly aria-label="Automatisch berechnete Versionsnummer" title="Wird automatisch aus bisherigen gültigen Versionen ermittelt"/></label><label>Beschreibung (optional)<input value={versionDescription} maxLength={2000} placeholder="Was ist neu?" onChange={e=>setVersionDescription(e.target.value)}/></label><button className="btn primary" disabled={versionBusy} onClick={()=>void releaseVersion()}><Save size={16}/> {versionBusy?'Sichere…':'Version freigeben'}</button></div><p className="muted version-rule">Major erhöht die Hauptversion und setzt Minor auf 0. Minor erhöht nur die Unterversion. Ältere, frei benannte Versionen bleiben bestehen, beeinflussen aber die automatische Nummerierung nicht.</p>
       <div className="snapshot-tip">Die Sicherung ist unveränderlich und speichert identische Dateiinhalte nur einmal. Alte Versionen werden hier noch nicht wiederhergestellt.</div>
       <h3>Versionshistorie ({versions.length})</h3>
       {versions.length?versions.map(v=><div className="version-entry" key={v.id}><button className="version-entry-head" onClick={()=>void toggleVersion(v.id)}><div><strong>{v.label}</strong><small>{date(v.createdAt)} · {v.fileCount} Dateien · {bytes(v.totalSizeBytes)}</small>{v.description&&<small>{v.description}</small>}</div>{expandedVersion===v.id?<ChevronUp size={17}/>:<ChevronDown size={17}/>}</button>{expandedVersion===v.id&&<div className="version-files">{snapshotFiles.map(f=><div className="version-file" key={f.relativePath}><span>{f.relativePath}</span><small>{bytes(f.sizeBytes)} · SHA-256 {f.sha256.slice(0,12)}…</small></div>)}</div>}</div>):<p className="muted">Noch keine freigegebene Version vorhanden.</p>}
      </section>),
        completeness: (<section className="panel metadata-panel checklist-panel">
       <div className="panel-heading"><div><h2>Vollständigkeit {checkInfo&&<span className="change-counter">{checkInfo.summary.percent}%</span>}</h2><p>Automatisch erkannte Dateien und manuelle Aufgaben. Der Projektstatus bleibt davon unabhängig.</p></div><button className="btn" aria-expanded={checkOpen} onClick={()=>{setCheckOpen(v=>!v);if(!checkOpen)void refreshChecklist(current.id).catch(e=>setError(String(e)))}}>{checkOpen?<ChevronUp size={15}/>:<ChevronDown size={15}/>} {checkOpen?'Einklappen':'Aufklappen'}</button></div>
       {checkOpen&&(checkInfo?<>
        <div className="completeness-progress"><div className="completeness-track"><span style={{width:checkInfo.summary.percent+'%'}}/></div><strong>{checkInfo.summary.done} / {checkInfo.summary.total} Pflichtpunkte erfüllt</strong></div>
        <p className="muted">Dateitypen werden automatisch geprüft. Pflichtdateien legst du unter Einstellungen fest. Eigene Aufgaben können hier als Pflicht markiert werden.</p>
        {checkInfo.items.map(item=><div key={item.id} className="checklist-item">
         <span className={"checkstate "+(item.completed?'done':'missing')}>{item.completed?'✓':'○'}</span>
         <div className="checklist-label"><strong>{item.label}</strong><small>{item.ruleType==='task'?'Manuelle Aufgabe':item.matchedFiles.length?item.matchedFiles.length+' passende Datei(en): '+item.matchedFiles.slice(0,2).join(', ')+(item.matchedFiles.length>2?' …':''):'Keine passende Datei gefunden'}</small></div>
         {item.ruleType==='task'&&<label className="checklist-small"><input type="checkbox" disabled={checkBusy} checked={item.completed} onChange={e=>void editCheck(item,'completed',e.target.checked)}/> Erledigt</label>}
         {item.ruleType==='task'?<label className="checklist-small"><input type="checkbox" disabled={checkBusy} checked={item.required} onChange={e=>void editCheck(item,'required',e.target.checked)}/> Pflicht</label>:<span className="muted">{item.required?'Pflichtdatei':'Optional'}</span>}
         {item.ruleType==='task'&&<button className="btn tiny" disabled={checkBusy} onClick={()=>void deleteTask(item.id)}>Entfernen</button>}
        </div>)}
        <div className="checklist-add"><input maxLength={160} value={checkTask} onChange={e=>setCheckTask(e.target.value)} placeholder="Weitere Aufgabe, z. B. Testdruck dokumentieren" onKeyDown={e=>{if(e.key==='Enter')void addTask()}}/><button className="btn" disabled={checkBusy||!checkTask.trim()} onClick={()=>void addTask()}><Plus size={15}/> Aufgabe hinzufügen</button></div>
        <p className="muted">Die Checkliste wird in SQLite gespeichert. Die Erfüllung von Datei-Anforderungen wird aus dem aktuellen Projektordner berechnet; Dateien werden nicht verändert.</p>
       </>:<p className="muted">Lade Checkliste…</p>)}
      </section>),
        metadata: (<section className="panel metadata-panel">
       <div className="panel-heading"><div><h2>Projektmetadaten</h2><p>Kategorie, Autor, Lizenz und Herkunft für dieses Projekt. Alle Angaben sind optional.</p></div><button className="btn" aria-expanded={detailsOpen} onClick={()=>setDetailsOpen(v=>!v)}>{detailsOpen?<ChevronUp size={15}/>:<ChevronDown size={15}/>} {detailsOpen?'Einklappen':'Aufklappen'}</button></div>
       {detailsOpen&&<><div className="metadata-grid">
         <label>Kategorie<input maxLength={120} value={details.category} placeholder="z. B. Funktionsteil" onChange={e=>setDetails(v=>({...v,category:e.target.value}))}/></label>
         <label>Autor / Ersteller<input maxLength={120} value={details.author} placeholder="Optional" onChange={e=>setDetails(v=>({...v,author:e.target.value}))}/></label>
         <label>Lizenz<input maxLength={120} value={details.license} placeholder="z. B. CC BY 4.0" onChange={e=>setDetails(v=>({...v,license:e.target.value}))}/></label>
         <label>Quelle (URL)<input maxLength={1000} type="url" value={details.sourceUrl} placeholder="https://…" onChange={e=>setDetails(v=>({...v,sourceUrl:e.target.value}))}/></label>
       </div><div className="buttons end"><button className="btn primary" disabled={detailsBusy} onClick={()=>void saveDetails()}><Save size={15}/> {detailsBusy?'Speichert…':'Metadaten speichern'}</button></div><p className="muted">Daten werden in SQLite und project.json abgelegt. Bereits vorhandene Projekte bleiben erhalten.</p></>}
      </section>),
        variants: (<section className="panel variant-panel">
       <div className="panel-heading"><div><h2>Projektvarianten ({variantInfo.variants.length})</h2><p>Einfarbig, mehrfarbig oder andere Ausführungen verwalten. Alle Varianten nutzen dieselben Projektordner und Stammdaten.</p></div></div>
       <div className="variant-form"><label>Variantenname<input maxLength={80} value={variantName} onChange={e=>setVariantName(e.target.value)} placeholder="z. B. Einfarbig"/></label><label>Beschreibung (optional)<input maxLength={1000} value={variantDesc} onChange={e=>setVariantDesc(e.target.value)} placeholder="z. B. Für PLA"/></label><button className="btn primary" disabled={variantBusy||!variantName.trim()} onClick={()=>void saveVariant()}>{editVariant?'Änderungen speichern':'＋ Variante anlegen'}</button>{editVariant&&<button className="btn" onClick={()=>{setEditVariant(null);setVariantName('');setVariantDesc('')}}>Abbrechen</button>}</div>
       {variantInfo.variants.length?<div className="variant-grid">{variantInfo.variants.map(v=><div className="variant-card" key={v.id}><div className="variant-card-head"><strong>{v.name}</strong><span className="muted">{variantInfo.links.filter(l=>l.variantId===v.id).length} Dateien</span></div><p>{v.description||'Keine zusätzliche Beschreibung'}</p><div className="buttons"><button className="btn tiny" disabled={variantBusy} onClick={()=>{setEditVariant(v.id);setVariantName(v.name);setVariantDesc(v.description)}}>Bearbeiten</button><button className="btn tiny" disabled={variantBusy} onClick={()=>void removeVariant(current.id,v.id,v.name)}>Löschen</button></div></div>)}</div>:<p className="muted">Noch keine Varianten vorhanden. Erstelle die erste Variante, um Dateien zuzuordnen.</p>}
       <p className="form-note">Dateizuordnung erfolgt direkt unten in der Dateiliste. Dateien ohne explizite Zuordnung gelten als gemeinsam und bleiben für alle Varianten verfügbar.</p>
      </section>),
        threeMf: (<section className="panel metadata-panel">
        <div className="panel-heading"><div><h2>3MF-Druckinformationen</h2><p>Beim Aufklappen automatisch auslesen. Bei mehreren Dateien zeigt ein Mouse-Hover die jeweiligen Werte.</p></div><button className="btn" aria-expanded={mfOpen} onClick={()=>setMfOpen(v=>!v)}>{mfOpen?<ChevronUp size={15}/>:<ChevronDown size={15}/>} {mfOpen?'Einklappen':'Aufklappen'}</button></div>
        {mfOpen&&<>
          {mfFiles.length===0?<p className="muted">Keine 3MF-Dateien vorhanden.</p>:<>
            {mfFiles.length>1&&<div className="mf-file-choices" aria-label="3MF-Dateien">
              {mfFiles.map(f=>{
                const info=mfResults[f.relativePath],failure=mfErrors[f.relativePath];
                return <div className="mf-file-choice" key={f.relativePath}>
                  <button type="button" className={'btn mf-file-select '+(mfSelected===f.relativePath?'selected':'')} aria-pressed={mfSelected===f.relativePath} onClick={()=>setMfSelected(f.relativePath)}><File size={14}/> <span>{f.relativePath}</span>{mfLoading.has(f.relativePath)?' · …':failure?' · !':''}</button>
                  <div className="mf-hover-info" role="tooltip">
                    <strong>{f.relativePath}</strong>
                    {info&&Object.keys(info.fields).length>0?Object.entries(info.fields).map(([key,value])=><div className="mf-hover-row" key={key}><span>{key}</span><b>{value}</b></div>):<p>{failure?`Analyse fehlgeschlagen: ${failure}`:mfLoading.has(f.relativePath)?'Druckinformationen werden geladen…':'Keine unterstützten Druckparameter gefunden.'}</p>}
                    {info?.warnings.map((w,i)=><p key={i}>{w}</p>)}
                  </div>
                </div>;
              })}
            </div>}
            {mfSelected&&<div className="metadata-results" aria-live="polite">
              <h3>{mfSelected}</h3>
              {mfLoading.has(mfSelected)?<p className="muted">Analysiere 3MF…</p>:mfErrors[mfSelected]?<p className="muted">Analyse fehlgeschlagen: {mfErrors[mfSelected]}</p>:mfInfo?<>
                {Object.entries(mfInfo.fields).map(([key,value])=><div className="info" key={key}><span>{key}</span><strong>{value}</strong></div>)}
                {mfInfo.sources.length>0&&<p className="muted">Datenquelle: {mfInfo.sources.join(', ')}</p>}
                {mfInfo.warnings.map((warning,i)=><p className="muted" key={i}>{warning}</p>)}
              </>:<p className="muted">Keine Druckinformationen verfügbar.</p>}
            </div>}
          </>}
        </>}
      </section>),
        files: (<section className="panel"><h2>Projektdateien</h2><p>STL, 3MF, Bilder und CAD-Dateien direkt aus dem Windows-Explorer in 3db ziehen. Bei gleichem Namen wird der ältere Inhalt VOR dem Ersetzen gesichert.</p>
       <div className={'dropzone '+(dragOver?'over':'')} aria-label="Dateien aus dem Explorer hier ablegen"><Upload size={22}/><strong>{dropBusy?'Dateien werden importiert…':dragOver?'Dateien hier ablegen':'Dateien hierher ziehen'}</strong><small>Dateien werden automatisch in Models, Printfiles, Images, Documents oder Other eingeordnet. Originale bleiben erhalten.</small></div>
       <div className="buttons space"><button className="btn" onClick={()=>void scanNow()}><RotateCw size={15}/> Aktualisieren</button><button className="btn" aria-expanded={fileAreaOpen} onClick={()=>setFileAreaOpen(v=>!v)}>{fileAreaOpen?<ChevronUp size={15}/>:<ChevronDown size={15}/>} {fileAreaOpen?"Dateiliste einklappen":"Dateiliste aufklappen"}</button>{fileAreaOpen&&files.length>0&&<button className="btn" onClick={()=>setFileHistoriesOpen(s=>s.size===files.length?new Set():new Set(files.map(f=>f.relativePath)))}>{fileHistoriesOpen.size===files.length?"Alle Historien einklappen":"Alle Historien aufklappen"}</button>}</div>
       {fileAreaOpen&&(files.length?files.map(f=><div className="file-item" key={f.relativePath}><div className="file-row"><div className="file-meta"><File size={17}/><div><strong>{f.relativePath}</strong><small>{bytes(f.sizeBytes)}</small></div></div><button className="btn tiny" onClick={()=>void action('open_project_file',{id:current.id,relativePath:f.relativePath})}><ExternalLink size={14}/> Öffnen</button></div>{variantInfo.variants.length>0&&<div className="variant-file-assign"><span className="muted">Varianten:</span>{variantInfo.variants.map(v=><label className="variant-check" key={v.id}><input type="checkbox" disabled={variantBusy} checked={variantInfo.links.some(l=>l.relativePath===f.relativePath&&l.variantId===v.id)} onChange={e=>void assignVariant(current.id,f.relativePath,v.id,e.target.checked)}/>{v.name}</label>)}{!variantInfo.links.some(l=>l.relativePath===f.relativePath)&&<span className="muted">Gemeinsam (alle)</span>}</div>}<div className="file-history"><button type="button" className="file-history-toggle" aria-expanded={fileHistoriesOpen.has(f.relativePath)} onClick={()=>setFileHistoriesOpen(old=>{const next=new Set(old);if(next.has(f.relativePath))next.delete(f.relativePath);else next.add(f.relativePath);return next})}><History size={13}/> {fileHistory[f.relativePath]?.length??0} gespeicherte Dateistände {fileHistoriesOpen.has(f.relativePath)?<ChevronUp size={14}/>:<ChevronDown size={14}/>}</button>{fileHistoriesOpen.has(f.relativePath)&&(fileHistory[f.relativePath]||[]).map((r,i)=><div className="file-revision" key={r.id}><span>{date(r.capturedAt)} · {bytes(r.sizeBytes)} · {r.sha256.slice(0,10)}…{i===0?' · zuletzt erkannt':''}</span><button className="btn tiny" onClick={()=>void restoreRevision(current.id,r.id)}><Copy size={13}/> Als Kopie herstellen</button></div>)}</div></div> ):<div className="empty small-empty">Noch keine Dateien vorhanden. Ziehe STL-, 3MF- oder andere Dateien direkt hierher.</div>)}
       <p className="muted" style={{marginTop:12}}>Externe Änderungen werden beim 15-Sekunden-Abgleich als neue Dateistände gesichert, sofern 3db den früheren Stand bereits erfasst hatte. Kein permanentes Backup jedes Speichervorgangs.</p>
      </section>),
        info: (<section className="panel"><h2>Projektinformationen</h2><div className="info"><span>Status</span><span className={'status '+current.status}>{statusNames[current.status]}</span></div><div className="info"><span>Ordner</span><code>{current.slug}</code></div><div className="info"><span>Erstellt</span><strong>{date(current.createdAt)}</strong></div><div className="info"><span>Geändert</span><strong>{date(current.updatedAt)}</strong></div><h3>Tags</h3><div className="tags">{current.tags.length?current.tags.map(t=><span className="tag" key={t}>{t}</span>):<span className="muted">Keine Tags</span>}</div><div className="separator"/><button className="btn" onClick={()=>void action('update_project',{id:current.id,input:{name:current.name,description:current.description,status:current.status,tags:current.tags},archived:!current.archived},current.archived?'Projekt wiederhergestellt.':'Projekt archiviert.')}><Archive size={15}/>{current.archived?'Wiederherstellen':'Projekt archivieren'}</button><p className="muted" style={{marginTop:12}}>Archivieren löscht keine Dateien.</p></section>)
       };
       const visible=panels.filter(p=>p.visible);
       const stack=(items:PanelConfig[],side:string)=><div className="detail-stack" key={side}>{items.map(p=><React.Fragment key={p.id}>{blocks[p.id]}</React.Fragment>)}</div>;
       return <div className={"custom-detail-columns "+(layoutColumns===1?"single":"double")}>
        {layoutColumns===1?stack(visible,'one'):<>{stack(visible.filter(p=>p.column===1),'left')}{stack(visible.filter(p=>p.column===2),'right')}</>}
       </div>;
      })()}
    </> : page==='dashboard'?<><div className="top"><div><h1>Dashboard</h1><p>Deine 3D-Druckprojekte auf einen Blick.</p></div><button className="btn primary" onClick={()=>{setForm({...defaultInput});setFormTagsText('');setModal('new')}}><Plus size={16}/> Neues Projekt</button></div>
      <div className="stats">{([{key:'all',title:'Alle Projekte',value:counts.all},{key:'working',title:'In Arbeit',value:counts.working},{key:'finished',title:'Fertig',value:counts.finished},{key:'idea',title:'In Planung',value:counts.idea}]).map(s=><button key={s.key} onClick={()=>setDashboardFilter(dashboardFilter===s.key?'all':s.key)} className={'stat '+(dashboardFilter===s.key?'selected':'')}><span>{s.title}</span><strong>{s.value}</strong><small>{dashboardFilter===s.key?'✓ Filter aktiv':'Zum Filtern anklicken'}</small></button>)}</div>
      <section className="panel"><div className="panel-heading"><div><h2>{dashboardFilter==='all'?'Deine Projekte':statusNames[dashboardFilter as Status]}</h2><p>{dashboard.length} Projekte angezeigt</p></div><button className="btn" onClick={()=>{setPage('projects');setDashboardFilter('all')}}>Projektbibliothek →</button></div>{dashboard.length?<div className="project-grid">{dashboard.map(card)}</div>:<div className="empty">Hier sind noch keine Projekte. Erstelle dein erstes Projekt!</div>}</section>
    </>:page==='projects'?<><div className="top"><div><h1>Projektbibliothek</h1><p>Durchsuche und organisiere deine Projekte.</p></div><button className="btn primary" onClick={()=>{setForm({...defaultInput});setFormTagsText('');setModal('new')}}><Plus size={16}/> Neues Projekt</button></div>
      <div className="filters"><div className="search"><Search size={17}/><input placeholder="Projekte und Tags suchen…" value={search} onChange={e=>setSearch(e.target.value)}/></div><select value={filter} onChange={e=>setFilter(e.target.value)}><option value="all">Alle Status</option>{Object.entries(statusNames).map(([key,val])=><option value={key} key={key}>{val}</option>)}</select><select aria-label="Vollständigkeit filtern" value={completenessFilter} onChange={e=>setCompletenessFilter(e.target.value)}><option value="all">Alle Vollständigkeiten</option><option value="complete">Vollständig</option><option value="incomplete">Unvollständig</option></select><select aria-label="Kategorie filtern" value={categoryFilter} onChange={e=>setCategoryFilter(e.target.value)}><option value="all">Alle Kategorien</option>{[...new Set(Object.values(categories).filter(Boolean))].sort((a,b)=>a.localeCompare(b,'de')).map(x=><option key={x} value={x}>{x}</option>)}</select><label className="check"><input type="checkbox" checked={includeArchived} onChange={e=>setIncludeArchived(e.target.checked)}/> Archivierte</label></div>
      <div className="muted count">{visible.length} Projekte</div>{visible.length?<div className="project-grid">{visible.map(card)}</div>:<div className="empty">Keine passenden Projekte gefunden.</div>}
    </>:page==='import'?<><div className="top"><div><h1>Projekt importieren</h1><p>Vorhandene Projektordner sicher als Kopie übernehmen.</p></div><button className="btn primary" disabled={importBusy} onClick={()=>void pickImportFolder()}><FolderOpen size={16}/> Quellordner auswählen</button></div>
      <section className="panel"><h2>1. Quelle analysieren</h2><p>3db liest den ausgewählten Ordner rekursiv aus. Symbolische Links werden übersprungen.</p><div className="path">{importSource||'Noch kein Quellordner ausgewählt'}</div>{importBusy&&<p>Prüfung läuft…</p>}
      {importPreview&&<><div className="import-stats"><span><strong>{importPreview.entries.length}</strong> Dateien</span><span><strong>{bytes(importPreview.totalSizeBytes)}</strong> Gesamtgröße</span><span><strong>{importPreview.skippedLinks}</strong> übersprungene Verknüpfungen</span></div>
      <h2>2. Zielordner prüfen</h2><p>Automatische Zuordnung nach Dateiendung. Unbekannte Dateien landen unter Other. Bei identischen Zielnamen bitte Kategorie oder Dateinamen anpassen.</p>
      <div className="import-scroll"><table className="import-table"><thead><tr><th>Quelldatei</th><th>Zielordner</th><th>Zieldateiname</th><th>Größe</th></tr></thead><tbody>{importPreview.entries.map(entry=>{
       const mapped=importMappings.find(m=>m.sourceRelativePath===entry.sourceRelativePath);
       const filename=entry.targetRelativePath.split('/').pop()||'';
       return <tr key={entry.sourceRelativePath}><td title={entry.sourceRelativePath}>{entry.sourceRelativePath}</td><td><select disabled={importBusy} value={entry.category} onChange={e=>void updateImportMapping(entry.sourceRelativePath,e.target.value,mapped?.targetName??null)}>{['Models','Printfiles','Images','Documents','Other'].map(x=><option key={x}>{x}</option>)}</select></td>
       <td><input aria-label={'Zieldateiname '+entry.sourceRelativePath} disabled={importBusy} defaultValue={mapped?.targetName??filename} key={entry.sourceRelativePath+'-'+(mapped?.targetName??filename)} onBlur={e=>{if(e.target.value!==filename)void updateImportMapping(entry.sourceRelativePath,entry.category,e.target.value)}}/></td><td>{bytes(entry.sizeBytes)}</td></tr>
      })}</tbody></table></div>
      {importPreview.conflicts.length>0&&<div className="alert error" style={{marginTop:12}}><div><strong>Namenskonflikte müssen vor dem Import gelöst werden:</strong>{importPreview.conflicts.map(c=><div key={c}>{c}</div>)}</div></div>}
      <h2 style={{marginTop:18}}>3. Neues Projekt erstellen</h2><div className="import-inputs"><label>Projektname *<input maxLength={120} value={importData.name} onChange={e=>setImportData({...importData,name:e.target.value})}/></label><label>Beschreibung<input maxLength={5000} value={importData.description} onChange={e=>setImportData({...importData,description:e.target.value})}/></label><label>Status<select value={importData.status} onChange={e=>setImportData({...importData,status:e.target.value as Status})}>{Object.entries(statusNames).map(([k,v])=><option key={k} value={k}>{v}</option>)}</select></label><label>Tags (Komma getrennt)<input value={importTagsText} onChange={e=>setImportTagsText(e.target.value)} placeholder="Büro, Funktionsteil, Laptop"/></label></div>
      <p className="form-note">Vorschau: Projects / [Projektname] / Models · Printfiles · Images · Documents · Other (bei Bedarf) · project.json. Die Kopie wird vor Übernahme geprüft; Originaldateien bleiben erhalten.</p>
      <div className="buttons end"><button className="btn primary" disabled={importBusy||importPreview.conflicts.length>0||!importData.name.trim()} onClick={()=>void commitImport()}><FolderInput size={16}/> Projekt als Kopie importieren</button></div></>}
      </section>
    </>:<><div className="top"><div><h1>Einstellungen</h1><p>Persönliche Darstellung und Projektablage.</p></div></div>
      <section className="panel settings"><h2>Darstellung</h2><label className="setting"><span><strong>Projektvorschaubilder anzeigen</strong><small>Ohne Bilder erscheinen kompaktere Karten.</small></span><input type="checkbox" checked={showPreviews} onChange={e=>setShowPreviews(e.target.checked)}/></label><label className="setting"><span><strong>Farbschema</strong><small>Systemmodus folgt der Windows-Einstellung.</small></span><select value={theme} onChange={e=>setTheme(e.target.value as typeof theme)}><option value="system">System</option><option value="dark">Dunkel</option><option value="light">Hell</option></select></label></section><section className="panel settings"><h2>Pflichtdateien für alle Projekte</h2><p>Diese Regeln gelten global für die Vollständigkeitsprüfung. Bestehende Projekte übernehmen Änderungen sofort; individuelle Aufgaben bleiben erhalten.</p>{globalRequirements.map(r=><label className="setting" key={r.kind}><span><strong>{({stl:'STL-Datei', '3mf':'3MF-Datei',image:'Projektbild',pdf:'PDF-Dokumentation'} as Record<string,string>)[r.kind]||r.kind}</strong><small>{r.required?'Wird für 100 % Vollständigkeit benötigt':'Optional, beeinflusst die Prozentzahl nicht'}</small></span><input type="checkbox" disabled={requirementsBusy} checked={r.required} onChange={e=>void changeRequirement(r.kind,e.target.checked)}/></label>)}</section>
      <section className="panel settings layout-settings"><h2>Projektübersicht gestalten</h2><p>Ziehe die Kacheln am Griff an ihre Wunschposition. In der zweispaltigen Ansicht kannst du sie direkt zwischen beiden Spalten verschieben. Keine Leerzeilen zwischen unterschiedlich hohen Kacheln.</p>
       <div className="setting"><strong>Ansicht</strong><select aria-label="Spaltenanzahl" value={layoutColumns} onChange={e=>setLayoutColumns(Number(e.target.value) as 1|2)}><option value={1}>Eine Spalte</option><option value={2}>Zwei Spalten</option></select></div>
       <div className={"layout-board "+(layoutColumns===1?"single":"double")}>
        {([1,2] as const).filter(c=>layoutColumns===2||c===1).map(column=><div className={'layout-drop-column '+(dragTarget==='column'+column?'drop-active':'')} key={column} data-layout-column={column}>
         <div className="layout-column-title">{layoutColumns===1?'Projektübersicht':column===1?'Linke Spalte':'Rechte Spalte'}</div>
         {panels.filter(p=>layoutColumns===1||p.column===column).map((p,i,list)=>{
          return <div key={p.id} data-layout-item={p.id} className={'layout-editor-item '+(dragPanel===p.id?'drag-source ':'')+(dragTarget===p.id?'drop-active':'')}>
           <button type="button" title="Mit gedrückter Maustaste verschieben" aria-label={`${panelNames[p.id]} verschieben`} className="draghandle" onPointerDown={e=>beginLayoutDrag(e,p.id)} onPointerMove={updateLayoutDrag} onPointerUp={finishLayoutDrag} onPointerCancel={cancelLayoutDrag}>⠿</button>
           <strong>{panelNames[p.id]}</strong>
           <label><input type="checkbox" checked={p.visible} onChange={e=>editPanel(p.id,{visible:e.target.checked})}/> Anzeigen</label>
           <button className="btn tiny" title="Eine Position nach oben" disabled={i===0} onClick={()=>movePanel(p.id,-1)}>↑</button>
           <button className="btn tiny" title="Eine Position nach unten" disabled={i===list.length-1} onClick={()=>movePanel(p.id,1)}>↓</button>
           {layoutColumns===2&&<button className="btn tiny" title="In andere Spalte verschieben" onClick={()=>placePanel(p.id,column===1?2:1)}>{column===1?'→':'←'}</button>}
          </div>})}
         <div className="layout-drop-hint">Kachel hier ablegen</div>
        </div>)}
       </div>
       <button className="btn" style={{marginTop:12}} onClick={()=>{setPanels(defaultPanels);setLayoutColumns(2)}}>Standardlayout wiederherstellen</button>
      </section>
      <section className="panel settings"><h2>Audit-Protokoll</h2><p>Lokale Diagnose für 3db. Erfasst Zeitstempel, Funktionsnamen und Erfolg oder Fehler – keine Dateinamen, Projektnamen oder Inhalte. Prüfe die Datei vor dem Weitergeben.</p><button className="btn" onClick={()=>void invoke('open_diagnostic_folder').catch(e=>setError(String(e)))}>📁 Log-Ordner im Explorer öffnen</button><p className="muted">Pfad: %LOCALAPPDATA%\3db\logs\3db-audit.log. Bei Bedarf ältere Datei 3db-audit.log.old beilegen.</p></section><section className="panel settings"><h2>Projektablage</h2><p>3db verwendet einen verwalteten Hauptordner.</p><div className="path">{libraryPath||'Wird beim Start ermittelt'}</div><p className="muted">Die Ablage kann in diesem ersten Prototyp noch nicht geändert werden.</p></section>
    </>}
   </main>
   {modal&&<div className="backdrop" onMouseDown={e=>{if(e.target===e.currentTarget)setModal(null)}}><div className="dialog" role="dialog" aria-modal="true" aria-labelledby="form-title"><div className="dialog-heading"><h2 id="form-title">{modal==='new'?'Neues Projekt':'Projekt bearbeiten'}</h2><button className="icon" onClick={()=>setModal(null)}><X size={18}/></button></div><form onSubmit={submit}><label>Projektname *<input autoFocus value={form.name} maxLength={120} onChange={e=>setForm({...form,name:e.target.value})} placeholder="z. B. Laptopständer" required/></label><label>Beschreibung<textarea value={form.description} maxLength={5000} onChange={e=>setForm({...form,description:e.target.value})} rows={3} placeholder="Was möchtest du drucken?"/></label><label>Status<select value={form.status} onChange={e=>setForm({...form,status:e.target.value as Status})}>{Object.entries(statusNames).map(([s,l])=><option key={s} value={s}>{l}</option>)}</select></label><label>Tags <span className="muted">(mit Komma trennen)</span><div className="tagsinput"><Tag size={16}/><input value={formTagsText} onChange={e=>setFormTagsText(e.target.value)} placeholder="Büro, BMW, Zubehör"/></div></label>{modal==='new'&&<p className="form-note">Es entstehen die Ordner Models, Printfiles, Images und Documents sowie eine project.json.</p>}<div className="buttons end"><button type="button" className="btn" onClick={()=>setModal(null)}>Abbrechen</button><button type="submit" className="btn primary" disabled={busy||!form.name.trim()}>{busy?'Speichern…':modal==='new'?'Projekt erstellen':'Speichern'}</button></div></form></div></div>}
 </div>;
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><App/></React.StrictMode>);
