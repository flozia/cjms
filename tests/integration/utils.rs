use fake::{Fake, StringFaker};
use lib::appconfig::{connect_to_database_and_migrate, run_server};
use lib::settings::{get_settings, Settings};

use lib::telemetry::StatsD;
use secrecy::{ExposeSecret, Secret};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, Executor, PgConnection, PgPool, Pool, Postgres};
use std::net::TcpListener;
use uuid::Uuid;

pub struct TestApp {
    pub settings: Settings,
}
impl TestApp {
    pub fn build_url(&self, path: &str) -> String {
        format!("http://{}{}", self.settings.server_address(), path)
    }

    pub fn db_connection(&self) -> PgPool {
        PgPoolOptions::new()
            .connect_timeout(std::time::Duration::from_secs(2))
            .connect_lazy(self.settings.database_url.expose_secret())
            .expect("Could not get DB connection for test")
    }
}

async fn create_test_database(database_url: &str) -> String {
    let randomized_test_database_url = format!("{}_test_{}", database_url, Uuid::new_v4());
    let url_parts: Vec<&str> = randomized_test_database_url.rsplit('/').collect();
    let database_name = url_parts.get(0).unwrap().to_string();
    let mut connection = PgConnection::connect(database_url)
        .await
        .expect("Failed to connect to postgres.");
    connection
        .execute(format!(r#"CREATE DATABASE "{}";"#, &database_name).as_str())
        .await
        .expect("Failed to create test database.");
    println!("Database is: {}", randomized_test_database_url);
    randomized_test_database_url
}

pub async fn get_test_db_pool() -> Pool<Postgres> {
    let settings = get_settings();
    let test_database_url = create_test_database(settings.database_url.expose_secret()).await;
    connect_to_database_and_migrate(&test_database_url).await
}

pub async fn spawn_app() -> TestApp {
    let mut settings = get_settings();
    let test_subid = random_simple_ascii_string();
    let test_aic_expiration_days = random_integer();
    let test_auth_password = random_ascii_string();
    let test_cj_signature = random_simple_ascii_string();
    let test_database_url = create_test_database(settings.database_url.expose_secret()).await;
    let listener =
        TcpListener::bind(format!("{}:0", settings.host)).expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();
    settings.aic_expiration_days = test_aic_expiration_days;
    settings.authentication = test_auth_password;
    settings.cj_signature = test_cj_signature;
    settings.cj_subid = test_subid;
    settings.database_url = Secret::new(test_database_url);
    settings.port = port;
    let statsd = StatsD::new(&settings);
    let db_pool = connect_to_database_and_migrate(settings.database_url.expose_secret()).await;
    let server =
        run_server(settings.clone(), listener, db_pool, statsd).expect("Failed to start server");
    let _ = tokio::spawn(server);
    TestApp { settings }
}

pub fn random_ascii_string() -> String {
    const ASCII: &str =
        "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#$%&\'()*+,-./:;<=>?@";
    let f = StringFaker::with(Vec::from(ASCII), 8..90);
    f.fake()
}

pub fn random_simple_ascii_string() -> String {
    const ASCII: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ._-";
    let f = StringFaker::with(Vec::from(ASCII), 8..90);
    f.fake()
}

pub fn random_currency_or_country() -> String {
    const LETTERS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let f = StringFaker::with(Vec::from(LETTERS), 1..5);
    f.fake()
}

pub fn random_integer() -> u64 {
    (1..200).fake()
}

pub fn random_price() -> i32 {
    (99..7899).fake()
}

pub async fn send_get_request(app: &TestApp, path: &str) -> reqwest::Response {
    let path = app.build_url(path);
    reqwest::get(&path).await.expect("Failed to GET")
}

pub async fn send_post_request(
    app: &TestApp,
    path: &str,
    data: serde_json::Value,
) -> reqwest::Response {
    let path = app.build_url(path);
    let client = reqwest::Client::new();
    client
        .post(&path)
        .json(&data)
        .send()
        .await
        .expect("Failed to POST")
}

pub async fn send_put_request(
    app: &TestApp,
    path: &str,
    data: serde_json::Value,
) -> reqwest::Response {
    let path = app.build_url(path);
    let client = reqwest::Client::new();
    client
        .put(&path)
        .json(&data)
        .send()
        .await
        .expect("Failed to PUT")
}
