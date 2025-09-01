#[macro_use]
extern crate rocket;

use anyhow::Context;
use rocket::State;

struct ServerSecretsState {
    bot_token: String,
}

#[get("/")]
fn index() -> &'static str {
    "Hi there"
}

#[post("/<token>", data = "<json_data>")]
fn handle_bot_updates(
    token: &str,
    json_data: String,
    secrets: &State<ServerSecretsState>,
) -> &'static str {
    println!("Received JSON: {}", json_data);
    println!("{} {}", token, secrets.bot_token);

    "hi!"
}

#[shuttle_runtime::main]
async fn main(
    #[shuttle_runtime::Secrets] secrets: shuttle_runtime::SecretStore,
) -> shuttle_rocket::ShuttleRocket {
    let bot_token = secrets.get("BOT_TOKEN").context("Bot token wasn't found");

    let server_secrets_state = ServerSecretsState {
        bot_token: bot_token.unwrap(),
    };
    let rocket = rocket::build()
        .mount("/", routes![index, handle_bot_updates])
        .manage(server_secrets_state);

    Ok(rocket.into())
}
