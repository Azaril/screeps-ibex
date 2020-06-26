use serde::*;
use clap::Clap;

#[derive(Clap)]
#[clap(version = "1.0", author = "William Archbell <william@archbell.com>")]
struct Opts {
    #[clap(short, long)]
    username: String,
    #[clap(short, long)]
    password: String,
    #[clap(long)]
    path: String,
}

#[derive(Serialize)]
struct AuthData {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct RemoveData {
    path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts: Opts = Opts::parse();

    let client = reqwest::Client::new();

    let auth_data = AuthData { username: opts.username, password: opts.password };

    let login_response = client.post("https://screepspl.us/api/auth/login")
        .json(&auth_data)
        .send()
        .await?;

    let auth_token = login_response
        .text()
        .await?;

    let remove_data = RemoveData { path: opts.path };

    let _remove_response = client.post("https://screepspl.us/api/stats/remove")
        .header("authorization", format!("JWT {}", auth_token))
        .json(&remove_data)
        .send()
        .await?;

    Ok(())
}