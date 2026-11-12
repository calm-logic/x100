use x100_proto::{ChatSent, RaiseTask, RpcError, SendChat, TaskRaised};

use super::refused;
use crate::hub::Hub;

pub async fn send(hub: &Hub, p: SendChat) -> Result<ChatSent, RpcError> {
    let message = hub
        .orch
        .send_chat(p.project, p.body, p.attachments, p.in_reply_to)
        .await
        .map_err(refused)?;
    Ok(ChatSent { message })
}

pub async fn raise(hub: &Hub, p: RaiseTask) -> Result<TaskRaised, RpcError> {
    let task = hub
        .orch
        .raise_task(p.project, p.title, p.ask, p.part, p.attachments)
        .await
        .map_err(refused)?;
    Ok(TaskRaised { task })
}
