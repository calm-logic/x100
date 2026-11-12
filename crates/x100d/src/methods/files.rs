use x100_core::event::ProjectEvent;
use x100_core::file::FileRef;
use x100_core::id::FileId;
use x100_core::party::{Actor, Party};
use x100_orch::new_id;
use x100_proto::{
    Bytes, Done, ErrorCode, FileContent, FileUploaded, ReadFile, RemoveFile, RpcError, UploadFile,
};

use super::{blocking, project, store_error};
use crate::hub::Hub;

pub async fn upload(hub: &Hub, p: UploadFile) -> Result<FileUploaded, RpcError> {
    let store = project(hub, p.project)?;
    if p.name.trim().is_empty() {
        return Err(RpcError::new(
            ErrorCode::InvalidParams,
            "a file needs a name",
        ));
    }
    let size = p.data.0.len() as u64;
    let files = store.files.clone();
    let bytes = p.data.0;
    let hash = blocking(move || files.put(&bytes))
        .await
        .map_err(|e| store_error(&e))?;
    let file = FileRef {
        id: new_id(&*hub.clock, FileId::new),
        name: p.name,
        media_type: p.media_type,
        size,
        hash,
        added_by: Party::Builder,
        // The message it goes with does not exist yet: the message lists it.
        attached_to: None,
    };
    let event = ProjectEvent::FileAdded(file.clone());
    blocking(move || store.log.append(Actor::Builder, event))
        .await
        .map_err(|e| store_error(&e))?;
    Ok(FileUploaded { file })
}

fn record(store: &x100_store::ProjectStore, id: FileId) -> Result<FileRef, RpcError> {
    store
        .log
        .with_state(|s| s.file(id).map(|f| f.file.clone()))
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, format!("no file {id}")))
}

pub async fn read(hub: &Hub, p: ReadFile) -> Result<FileContent, RpcError> {
    let store = project(hub, p.project)?;
    let file = record(&store, p.file)?;
    let (files, hash) = (store.files.clone(), file.hash.clone());
    let data = blocking(move || files.read(&hash))
        .await
        .map_err(|e| store_error(&e))?;
    Ok(FileContent {
        file,
        data: Bytes(data),
    })
}

pub async fn remove(hub: &Hub, p: RemoveFile) -> Result<Done, RpcError> {
    let store = project(hub, p.project)?;
    let file = record(&store, p.file)?;
    blocking(move || {
        store
            .log
            .append(Actor::Builder, ProjectEvent::FileRemoved { file: p.file })?;
        // The same bytes may be another file's, since files are stored by content.
        let shared = store
            .log
            .with_state(|s| s.files().any(|f| f.file.hash == file.hash));
        if !shared {
            store.files.remove(&file.hash)?;
        }
        Ok(())
    })
    .await
    .map_err(|e: x100_store::Error| store_error(&e))?;
    Ok(Done {})
}
