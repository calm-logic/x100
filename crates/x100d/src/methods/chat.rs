use x100_orch::{raise_task, send_brief, send_chat};
use x100_proto::{BriefSent, ChatSent, RaiseTask, RpcError, SendBrief, SendChat, TaskRaised};

use super::{project, refused};
use crate::hub::Hub;

pub async fn brief(hub: &Hub, p: SendBrief) -> Result<BriefSent, RpcError> {
    let store = project(hub, p.project)?;
    let (message, task) = send_brief(&store, &*hub.clock, p.body, p.attachments)
        .await
        .map_err(refused)?;
    Ok(BriefSent { message, task })
}

pub async fn send(hub: &Hub, p: SendChat) -> Result<ChatSent, RpcError> {
    let store = project(hub, p.project)?;
    let message = send_chat(&store, &*hub.clock, p.body, p.attachments, p.in_reply_to)
        .await
        .map_err(refused)?;
    Ok(ChatSent { message })
}

pub async fn raise(hub: &Hub, p: RaiseTask) -> Result<TaskRaised, RpcError> {
    let store = project(hub, p.project)?;
    let task = raise_task(&store, &*hub.clock, p.title, p.ask, p.part, p.attachments)
        .await
        .map_err(refused)?;
    Ok(TaskRaised { task })
}
