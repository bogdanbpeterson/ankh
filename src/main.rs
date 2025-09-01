#[macro_use]
extern crate rocket;

#[get("/")]
fn index() -> &'static str {
    "Hi there"
}

#[shuttle_runtime::main]
async fn main() -> shuttle_rocket::ShuttleRocket {
    let rocket = rocket::build().mount("/", routes![index]);

    Ok(rocket.into())
}
