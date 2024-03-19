#[derive(Debug)]
pub enum MessageType {
    TwitchStreamOnline,
    TwitchStreamOffline,
}

#[derive(Debug)]
pub struct InterComm {
    pub message_type: MessageType,
    pub streamer_user_id: String,
    pub streamer_user_login: String,
}
