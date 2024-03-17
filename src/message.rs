pub enum MessageType {
    TwitchStreamOnline,
    TwitchStreamOffline,
}

pub struct Message {
    pub message_type: MessageType,
    pub streamer: String,
}
