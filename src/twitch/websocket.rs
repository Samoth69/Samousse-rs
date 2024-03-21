use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use anyhow::Context;
use futures::{stream, TryStreamExt};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn, Instrument};
use twitch_api::client::ClientDefault;
use twitch_api::eventsub::stream::{StreamOfflineV1, StreamOnlineV1};
use twitch_api::eventsub::{
    Event, EventSubSubscription, EventSubscription, EventType, EventsubWebsocketData, Message,
    ReconnectPayload, SessionData, WelcomePayload,
};
use twitch_api::helix::eventsub::EventSubSubscriptions;
use twitch_api::types::{EventSubId, UserId, UserName};
use twitch_api::{eventsub, HelixClient};
use twitch_oauth2::UserToken;

use crate::config::TwitchWatcher;
use crate::inter_comm::{InterComm, MessageType};
use crate::twitch::auth::{get_client_ids, TwitchToken};

pub async fn run(sender: Sender<InterComm>, config: &TwitchWatcher) -> anyhow::Result<()> {
    let twitch_client: HelixClient<_> = HelixClient::with_client(
        <reqwest::Client>::default_client_with_name(Some("samousse-rs".parse()?))?,
    );

    let mut ws = WebsocketClient {
        sender,
        session_id: None,
        token: TwitchToken::new().await?,
        client: twitch_client,
        user_ids: config
            .channels
            .iter()
            .map(|i| UserId::new(i.twitch_channel_id.to_string()))
            .collect(),
        connect_url: twitch_api::TWITCH_EVENTSUB_WEBSOCKET_URL.clone(),
        event_sub_id: vec![],
    };

    ws.run().await?;

    Ok(())
}

#[derive(Clone)]
pub struct Subscription {
    user_id: UserId,
    event_id: Option<EventSubId>,
    event_type: EventType,
}

pub struct WebsocketClient {
    sender: Sender<InterComm>,
    user_ids: Vec<UserId>,

    /// The session id of the websocket connection
    session_id: Option<String>,
    /// The token used to authenticate with the Twitch API
    token: TwitchToken,
    /// The client used to make requests to the Twitch API
    client: HelixClient<'static, reqwest::Client>,
    /// The url to use for websocket
    connect_url: url::Url,
    /// contain the current of subscriptions in twitch api
    event_sub_id: Vec<Subscription>,
}

impl WebsocketClient {
    async fn connect(
        &self,
    ) -> anyhow::Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let socket_config = tungstenite::protocol::WebSocketConfig::default();
        info!("connecting to websocket");
        let (socket, _) = tokio_tungstenite::connect_async_with_config(
            &self.connect_url,
            Some(socket_config),
            false,
        )
        .await
        .context("Can't connect")?;

        Ok(socket)
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        let mut s = self.connect().await?;

        loop {
            tokio::select!(
                Some(msg) = futures::StreamExt::next(&mut s) => {
                    let msg = match msg {
                        Err(tungstenite::Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake)) => {
                            warn!("connection was sent an unexpected frame or was reset, reestablishing it");
                            s = self.connect().await.context("when reestablishing connection")?;
                            continue;
                        }
                        _=> msg.context("when getting message")?,
                    };
                    self.process_message(msg).await?
                }
                else => {
                    warn!("Twitch websocket loop exited, waiting before restart");
                    sleep(Duration::from_secs(30)).await;
                }
            )
        }
    }

    /// Process a message from the websocket
    pub async fn process_message(&mut self, msg: tungstenite::Message) -> anyhow::Result<()> {
        trace!("processing");
        match msg {
            tungstenite::Message::Text(s) => {
                // Parse the message into a [twitch_api::eventsub::EventsubWebsocketData]
                match Event::parse_websocket(&s)? {
                    EventsubWebsocketData::Welcome {
                        payload: WelcomePayload { session },
                        ..
                    }
                    | EventsubWebsocketData::Reconnect {
                        payload: ReconnectPayload { session },
                        ..
                    } => match self.process_welcome_message(session).await {
                        Err(e) => {
                            error!("Error on processing welcome message : {}", e);
                            Ok(())
                        }
                        _ => Ok(()),
                    },
                    // Here is where you would handle the events you want to listen to
                    EventsubWebsocketData::Notification {
                        metadata: _,
                        payload,
                    } => {
                        match payload {
                            Event::StreamOnlineV1(eventsub::Payload {
                                message: Message::Notification(notif),
                                ..
                            }) => {
                                self.handle_streamer_online(
                                    notif.broadcaster_user_id,
                                    notif.broadcaster_user_login,
                                )
                                .await?;
                            }
                            Event::StreamOfflineV1(eventsub::Payload {
                                message: Message::Notification(notif),
                                ..
                            }) => {
                                self.handle_streamer_offline(
                                    notif.broadcaster_user_id,
                                    notif.broadcaster_user_login,
                                )
                                .await?;
                            }
                            _ => {}
                        }
                        Ok(())
                    }
                    EventsubWebsocketData::Revocation {
                        metadata,
                        payload: _,
                    } => {
                        info!("got revocation event: {metadata:?}");
                        Ok(())
                    }
                    EventsubWebsocketData::Keepalive {
                        metadata: _,
                        payload: _,
                    } => {
                        trace!("Received keepalive");
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            tungstenite::Message::Close(_) => {
                info!("Received close event");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub async fn process_welcome_message(&mut self, data: SessionData<'_>) -> anyhow::Result<()> {
        self.session_id = Some(data.id.to_string());
        if let Some(url) = data.reconnect_url {
            self.connect_url = url.parse()?;
        }

        let transport = eventsub::Transport::websocket(data.id.clone());
        let cred = get_client_ids();
        let token = UserToken::from_existing_unchecked(
            self.token.access_token.to_owned(),
            Some(self.token.refresh_token.to_owned().into()),
            cred.0,
            Some(cred.1.into()),
            "samoth691".into(),
            "53102824".into(),
            None,
            None,
        );

        // ---------------------------------------------------------------------------
        // We find what event we already have a sub for
        // ---------------------------------------------------------------------------
        // https://github.com/twitch-rs/twitch_api/issues/400
        let subs: Vec<EventSubSubscription> = self
            .client
            .get_eventsub_subscriptions(None, None, None, &token)
            .map_ok(|r| {
                trace!("{:?}", r);
                stream::iter(
                    r.subscriptions
                        .into_iter()
                        .map(Ok::<_, twitch_api::helix::ClientRequestError<_>>),
                )
            })
            .try_flatten()
            .try_collect()
            .await?;
        // let subs: Vec<EventSubSubscription> = vec![];

        debug!("There are {} subs on twitch api side", subs.len());

        // ---------------------------------------------------------------------------
        // clearing the list for now
        // ---------------------------------------------------------------------------
        self.event_sub_id.clear();

        // ---------------------------------------------------------------------------
        // add all the event we need to have
        // ---------------------------------------------------------------------------
        for user_id in &self.user_ids {
            debug!("Subscribing to events for {}", user_id);
            let mut sub = Subscription {
                event_id: None,
                event_type: EventType::StreamOnline,
                user_id: user_id.clone(),
            };
            self.event_sub_id.push(sub.clone());

            sub.event_type = EventType::StreamOffline;
            self.event_sub_id.push(sub);
        }

        // ---------------------------------------------------------------------------
        // find event that are already subscribed
        // ---------------------------------------------------------------------------
        for sub in subs {
            if let Some(item) = self.event_sub_id.iter_mut().find(|f| {
                f.event_type == sub.type_
                    && f.user_id
                        == UserId::new(
                            sub.condition
                                .get("broadcaster_user_id")
                                .unwrap()
                                .to_string(),
                        )
            }) {
                item.event_id = Some(sub.id);
            } else {
                debug!("deleting old sub {}", sub.id);
                self.client
                    .delete_eventsub_subscription(sub.id, &token)
                    .await?;
            }
        }

        // ---------------------------------------------------------------------------
        // add sub for missing events
        // ---------------------------------------------------------------------------
        for to_sub in self
            .event_sub_id
            .iter_mut()
            .filter(|f| f.event_id.is_none())
        {
            let event = match to_sub.event_type {
                EventType::StreamOnline => {
                    self.client
                        .create_eventsub_subscription(
                            StreamOnlineV1::broadcaster_user_id(to_sub.user_id.clone()),
                            transport.clone(),
                            &token,
                        )
                        .await?
                        .id
                }
                EventType::StreamOffline => {
                    self.client
                        .create_eventsub_subscription(
                            StreamOfflineV1::broadcaster_user_id(to_sub.user_id.clone()),
                            transport.clone(),
                            &token,
                        )
                        .await?
                        .id
                }
                _ => {
                    panic!("Unknown event type")
                }
            };
            to_sub.event_id = Some(event);
        }

        info!("welcome message sent");
        Ok(())
    }

    pub async fn handle_streamer_online(
        &self,
        broadcaster_user_id: UserId,
        broadcaster_user_login: UserName,
    ) -> anyhow::Result<()> {
        info!("{} stream is online", broadcaster_user_login);

        self.sender
            .send(InterComm {
                message_type: MessageType::TwitchStreamOnline,
                streamer_user_id: broadcaster_user_id.into(),
                streamer_user_login: broadcaster_user_login.into(),
            })
            .await?;

        Ok(())
    }
    pub async fn handle_streamer_offline(
        &self,
        broadcaster_user_id: UserId,
        broadcaster_user_login: UserName,
    ) -> anyhow::Result<()> {
        info!("{} stream is offline", broadcaster_user_login);

        self.sender
            .send(InterComm {
                message_type: MessageType::TwitchStreamOffline,
                streamer_user_id: broadcaster_user_id.into(),
                streamer_user_login: broadcaster_user_login.into(),
            })
            .await?;

        Ok(())
    }
}
