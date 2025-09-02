use anyhow::Context;
use rocket::{State, get, post, routes, serde::json::Json};
use shuttle_rocket::ShuttleRocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use teloxide::{
    Bot,
    prelude::*,
    types::{ChatId, FileId, InputFile, ParseMode, Update},
};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant, sleep};
use url::Url;

#[derive(Clone)]
struct QueuedMessage {
    audio_file_id: FileId,
    message_id: i32,
}

struct MessageQueue {
    messages: Arc<Mutex<Vec<QueuedMessage>>>,
    last_received: Arc<Mutex<Instant>>,
    processing: Arc<Mutex<bool>>,
}

impl MessageQueue {
    fn new() -> Self {
        Self {
            messages: Arc::new(Mutex::new(Vec::new())),
            last_received: Arc::new(Mutex::new(Instant::now())),
            processing: Arc::new(Mutex::new(false)),
        }
    }

    async fn add_message(
        &self,
        audio_file_id: FileId,
        message_id: i32,
        bot: Arc<Bot>,
        secrets: Arc<ServerSecretsState>,
    ) {
        {
            let mut messages = self.messages.lock().await;
            let new_message = QueuedMessage {
                audio_file_id,
                message_id,
            };

            match messages.binary_search_by_key(&message_id, |m| m.message_id) {
                Ok(pos) => {
                    messages[pos] = new_message;
                }
                Err(pos) => {
                    messages.insert(pos, new_message);
                }
            }
        }

        *self.last_received.lock().await = Instant::now();

        {
            let mut processing = self.processing.lock().await;
            if !*processing {
                *processing = true;
                drop(processing);
                self.start_processing_task(bot, secrets).await;
            }
        }
    }

    async fn start_processing_task(&self, bot: Arc<Bot>, secrets: Arc<ServerSecretsState>) {
        let messages = self.messages.clone();
        let last_received = self.last_received.clone();
        let processing_flag = self.processing.clone();

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(3)).await;

                let time_since_last = last_received.lock().await.elapsed();
                if time_since_last < Duration::from_secs(3) {
                    continue;
                }

                let mut msgs = messages.lock().await;
                if msgs.is_empty() {
                    break;
                }

                let to_process = msgs.drain(..).collect::<Vec<_>>();
                drop(msgs);

                println!("Processing {} queued messages", to_process.len());

                let total_count = to_process.len();
                for (i, msg) in to_process.into_iter().enumerate() {
                    if let Err(e) = Self::send_audio_message(&bot, &secrets, &msg).await {
                        eprintln!("Error sending queued message: {}", e);
                    }

                    if i < total_count - 1 {
                        sleep(Duration::from_millis(1000)).await;
                    }
                }

                break;
            }

            *processing_flag.lock().await = false;
        });
    }

    async fn send_audio_message(
        bot: &Bot,
        secrets: &ServerSecretsState,
        queued_msg: &QueuedMessage,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let predicted_id = secrets.last_message_id.load(Ordering::Relaxed) + 1;

        let sent_message = bot
            .send_audio(
                ChatId(secrets.channel_id.parse()?),
                InputFile::file_id(queued_msg.audio_file_id.clone()),
            )
            .caption(format!(
                "[Music: Reborn](https://t.me/the_ankh_music/{})",
                predicted_id
            ))
            .parse_mode(ParseMode::MarkdownV2)
            .await?;

        if sent_message.id.0 == predicted_id {
            println!("Message ID prediction correct: {}", predicted_id);
            secrets
                .last_message_id
                .store(predicted_id, Ordering::Relaxed);
        } else {
            println!(
                "Message ID mismatch! Predicted: {}, Actual: {}",
                predicted_id, sent_message.id.0
            );

            bot.edit_message_caption(sent_message.chat.id, sent_message.id)
                .caption(format!(
                    "[Music: Reborn](https://t.me/the_ankh_music/{})",
                    sent_message.id.0
                ))
                .parse_mode(ParseMode::MarkdownV2)
                .await?;

            secrets
                .last_message_id
                .store(sent_message.id.0, Ordering::Relaxed);
        }

        Ok(())
    }
}

struct ServerSecretsState {
    bot_token: String,
    me_id: String,
    channel_id: String,
    last_message_id: AtomicI32,
    message_queue: MessageQueue,
}

async fn handle_update(
    bot: Arc<Bot>,
    update: Update,
    secrets: Arc<ServerSecretsState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let teloxide::types::UpdateKind::Message(message) = update.kind {
        if message.chat.id != ChatId(secrets.me_id.parse()?) {
            bot.send_message(
                message.chat.id,
                format!(
                    "Someone tried to use this bot {}",
                    message
                        .chat
                        .username()
                        .unwrap_or(&message.chat.id.to_string())
                ),
            )
            .await?;
            return Ok(());
        }

        if let Some(text) = message.text() {
            if text == "/start" {
                bot.send_message(message.chat.id, "Welcome! Up and running.")
                    .await?;
                return Ok(());
            }
        }

        if let Some(audio) = message.audio() {
            secrets
                .message_queue
                .add_message(
                    audio.file.id.clone(),
                    message.id.0,
                    bot.clone(),
                    secrets.clone(),
                )
                .await;

            println!("Added audio to queue (ID: {})", message.id.0);
        }

        bot.delete_message(message.chat.id, message.id).await?;
    }
    Ok(())
}

#[get("/")]
fn index_handler() -> &'static str {
    "hi!"
}

#[post("/<_bot_token>", data = "<update>")]
async fn webhook_handler(
    bot: &State<Arc<Bot>>,
    update: Json<Update>,
    _bot_token: &str,
    secrets: &State<Arc<ServerSecretsState>>,
) -> &'static str {
    let bot = bot.inner().clone();
    let secrets = secrets.inner().clone();
    tokio::spawn(async move {
        if let Err(e) = handle_update(bot, update.into_inner(), secrets).await {
            eprintln!("Error handling update: {}", e);
        }
    });
    "OK"
}

#[shuttle_runtime::main]
async fn main(#[shuttle_runtime::Secrets] secrets: shuttle_runtime::SecretStore) -> ShuttleRocket {
    let bot_token = secrets
        .get("BOT_TOKEN")
        .context("BOT_TOKEN environment variable must be set")?;
    let me_id = secrets
        .get("ME_ID")
        .context("ME_ID environment variable must be set")?;
    let channel_id = secrets
        .get("CHANNEL_ID")
        .context("CHANNEL_ID environment variable must be set")?;
    let public_url = secrets
        .get("PUBLIC_URL")
        .context("PUBLIC_URL must be set")?;

    let server_secrets_state = Arc::new(ServerSecretsState {
        bot_token,
        me_id,
        channel_id,
        last_message_id: AtomicI32::new(0),
        message_queue: MessageQueue::new(),
    });

    let bot = Arc::new(Bot::new(server_secrets_state.bot_token.clone()));
    let webhook_url = format!("{}/{}", public_url, server_secrets_state.bot_token);

    bot.set_webhook(Url::parse(&webhook_url).context("Failed to parse webhook URL")?)
        .await
        .context("Failed to set webhook")?;
    println!("Webhook set successfully");

    let rocket = rocket::build()
        .manage(bot)
        .mount("/", routes![index_handler, webhook_handler])
        .manage(server_secrets_state);
    Ok(rocket.into())
}
