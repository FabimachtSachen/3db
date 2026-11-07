# Data Management and Versioning

3db organizes projects in managed folders and stores structured metadata locally.

## Project files

Project content can include models, print files, images, and documents.

## Variants

A project may contain multiple variants. Files can be associated with specific variants.

## Snapshots

Version snapshots preserve project states and are intended to support traceability of changes.

## Backup guidance

- Back up the complete project library and associated database.
- Keep backups independently from the working directory.
- Do not modify database files manually while the application is running.
- Test restoration using a separate location before replacing working data.

## Compatibility

Historical prototype formats may require a dedicated migration procedure.

Do not assume every older data format supports an automatic in-place upgrade.
