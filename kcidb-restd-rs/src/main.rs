// SPDX-License-Identifier: LGPL-2.1-only
// Copyright (C) 2025 Collabora Ltd
// Author: Denys Fedoryshchenko <denys.f@collabora.com>
//
// This library is free software; you can redistribute it and/or modify it under
// the terms of the GNU Lesser General Public License as published by the Free
// Software Foundation; version 2.1.
//
// This library is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
// PARTICULAR PURPOSE. See the GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License along
// with this library; if not, write to the Free Software Foundation, Inc.,
// 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA

/*
KCIDB-Rust REST submissions receiver

1)Verify user authentication
2)Create file name with suffix _temp, until it is ready to be
   processed
3)After all file received, rename the file to the final name
4)Validate if the submission is valid JSON
* Optionally validate some other things

*/

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum_server::tls_rustls::RustlsConfig;
use clap::Parser;
use jsonwebtoken::{DecodingKey, Validation, decode};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tower_http::limit::RequestBodyLimitLayer;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, OrderStatus, RetryPolicy,
};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// The port to listen on
    #[clap(short, long, default_value = "0")]
    port: u16,
    /// The host to listen on
    #[clap(short = 'b', long, default_value = "0.0.0.0")]
    host: String,
    /// The path to the directory to store the received files
    #[clap(short = 'd', long, default_value = "/app/spool")]
    directory: String,
    /// JWT secret
    #[clap(short, long, default_value = "secret")]
    jwt_secret: String,
    /// Unified JWT secret (secondary, tried if primary fails)
    #[clap(long, default_value = "")]
    unified_secret: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SubmissionStatus {
    id: String,
    status: String,
    message: Option<String>,
}

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

struct AppState {
    directory: String,
    jwt_secret: String,
    unified_secret: String,
    submission_counter: AtomicU64,
    submission_size_total: AtomicU64,
    error_counter: AtomicU64,
    system_error_counter: AtomicU64,
    user_error_counter: AtomicU64,
    start_time: std::time::Instant,
    origin_counters: Mutex<HashMap<String, u64>>,
    /// HTTP-01 challenge responses (token -> key authorization) published by
    /// the built-in ACME renewal flow and served on /.well-known/acme-challenge.
    acme_challenges: Mutex<HashMap<String, String>>,
}

/// Normalize origin name to be safe for Prometheus labels:
/// lowercase, replace non-alphanumeric with underscore, collapse repeats
fn normalize_origin(origin: &str) -> String {
    let normalized: String = origin
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    // Collapse consecutive underscores
    let mut result = String::with_capacity(normalized.len());
    let mut prev_underscore = false;
    for c in normalized.chars() {
        if c == '_' {
            if !prev_underscore {
                result.push(c);
            }
            prev_underscore = true;
        } else {
            result.push(c);
            prev_underscore = false;
        }
    }
    result.trim_matches('_').to_string()
}

fn verify_submission_path(path: &str) -> bool {
    let path = Path::new(path);
    path.exists() && path.is_dir()
}

fn wait_for_file(path: &str) -> bool {
    let path = Path::new(path);
    // wait for the file to be created
    for _ in 0..300 {
        if path.exists() {
            return true;
        }
        println!("Waiting for file {} to be created", path.display());
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }
    false
}

async fn submission_metrics(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let spool_path = Path::new(&state.directory);
    let json_files_num = match spool_path.read_dir() {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and( |ext| ext == "json"))
            .count(),
        Err(_) => 0,
    };
    // Prometheus metrics format
    // String to hold the metrics
    let mut metrics = String::new();
    metrics.push_str("# HELP kcidb_submissions_total Total number of submissions received\n");
    metrics.push_str("# TYPE kcidb_submissions_total counter\n");
    metrics.push_str(&format!(
        "kcidb_submissions_total {}\n",
        state.submission_counter.load(Ordering::Relaxed)
    ));
    metrics.push_str(
        "# HELP kcidb_submission_size_total Total size of all submissions received in bytes\n",
    );
    metrics.push_str("# TYPE kcidb_submission_size_total counter\n");
    metrics.push_str(&format!(
        "kcidb_submission_size_total {}\n",
        state.submission_size_total.load(Ordering::Relaxed)
    ));
    metrics.push_str("# HELP kcidb_errors_total Total number of errors encountered\n");
    metrics.push_str("# TYPE kcidb_errors_total counter\n");
    metrics.push_str(&format!(
        "kcidb_errors_total {}\n",
        state.error_counter.load(Ordering::Relaxed)
    ));
    metrics.push_str("# HELP kcidb_system_errors_total Total number of system errors encountered\n");
    metrics.push_str("# TYPE kcidb_system_errors_total counter\n");
    metrics.push_str(&format!(
        "kcidb_system_errors_total {}\n",
        state.system_error_counter.load(Ordering::Relaxed)
    ));
    metrics.push_str("# HELP kcidb_user_errors_total Total number of user errors encountered\n");
    metrics.push_str("# TYPE kcidb_user_errors_total counter\n");
    metrics.push_str(&format!(
        "kcidb_user_errors_total {}\n",
        state.user_error_counter.load(Ordering::Relaxed)
    ));
    // number of json files in the spool directory
    metrics.push_str(
        "# HELP kcidb_json_files Total number of JSON files in the spool directory\n",
    );
    metrics.push_str("# TYPE kcidb_json_files gauge\n");
    metrics.push_str(&format!("kcidb_json_files {}\n", json_files_num));
    // Per-origin submission counts
    metrics.push_str("# HELP kcidb_submissions_by_origin Total submissions received per origin\n");
    metrics.push_str("# TYPE kcidb_submissions_by_origin counter\n");
    if let Ok(counters) = state.origin_counters.lock() {
        let mut origins: Vec<(&String, &u64)> = counters.iter().collect();
        origins.sort_by_key(|(k, _)| (*k).clone());
        for (origin, count) in origins {
            metrics.push_str(&format!(
                "kcidb_submissions_by_origin{{origin=\"{}\"}} {}\n",
                origin, count
            ));
        }
    }

    // Uptime in seconds
    let uptime = state.start_time.elapsed().as_secs();
    metrics.push_str("# HELP kcidb_uptime_seconds Uptime of the server in seconds\n");
    metrics.push_str("# TYPE kcidb_uptime_seconds gauge\n");
    metrics.push_str(&format!("kcidb_uptime_seconds {}\n", uptime));

    (StatusCode::OK, metrics)
}

fn are_we_root() -> bool {
    // Check if running as root (uid 0) without using unsafe
    #[cfg(target_family = "unix")]
    {
        match nix::unistd::Uid::effective().is_root() {
            true => true,
            false => false,
        }
    }
    #[cfg(not(target_family = "unix"))]
    {
        // On non-Unix platforms, always return false
        false
    }
}

async fn handle_root() -> impl IntoResponse {
    let index_path = Path::new("/usr/local/share/kcidb-restd-rs/index.html");
    let html = tokio::fs::read_to_string(index_path).await.unwrap_or_else(|_| {
        // Fallback HTML if the file is not found
        "<html><body><h1>Welcome to KCIDB REST API</h1></body></html>".to_string()
    });
    (
        StatusCode::OK,
        axum::response::Html(html)
    )
}

async fn serve_acme_challenge(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> impl IntoResponse {
    // Validate token contains only safe characters (alphanumeric, dash, underscore)
    if !token.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return (StatusCode::BAD_REQUEST, "Invalid token".to_string());
    }

    // Built-in ACME renewal publishes challenge responses in memory; serve those first.
    if let Ok(map) = state.acme_challenges.lock()
        && let Some(key_authorization) = map.get(&token)
    {
        return (StatusCode::OK, key_authorization.clone());
    }

    // TODO: Remove this deprecated feature on next code cleanup.
    // Fall back to a webroot directory (external certbot in webroot mode).
    let acme_webroot = match std::env::var("ACME_WEBROOT") {
        Ok(path) => path,
        Err(_) => {
            return (StatusCode::NOT_FOUND, "Challenge not found".to_string());
        }
    };

    let challenge_path = format!("{}/{}", acme_webroot, token);
    match tokio::fs::read_to_string(&challenge_path).await {
        Ok(content) => (StatusCode::OK, content),
        Err(_) => (StatusCode::NOT_FOUND, "Challenge not found".to_string()),
    }
}

async fn auth_test(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let auth_result = verify_auth(headers, state.clone());
    match auth_result {
        Ok(jwt) => {
            println!("Authentication successful for origin: {}", jwt.origin);
        }
        Err(e) => {
            println!("Error: {}", e);
            state.user_error_counter.fetch_add(1, Ordering::Relaxed);
            let jsanswer = generate_answer("error", "0", Some(e));
            return (StatusCode::UNAUTHORIZED, jsanswer);
        }
    }
    let jsanswer = generate_answer("ok", "0", Some("Authentication successful".to_string()));
    (StatusCode::OK, jsanswer) 
}

#[tokio::main]
async fn main() {
    let limit_layer = RequestBodyLimitLayer::new(512 * 1024 * 1024);
    let args = Args::parse();
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| args.jwt_secret.clone());
    let unified_secret = std::env::var("UNIFIED_SECRET").unwrap_or_else(|_| args.unified_secret.clone());
    let app_state = Arc::new(AppState {
        directory: args.directory,
        jwt_secret,
        unified_secret,
        submission_counter: AtomicU64::new(0),
        submission_size_total: AtomicU64::new(0),
        error_counter: AtomicU64::new(0),
        system_error_counter: AtomicU64::new(0),
        user_error_counter: AtomicU64::new(0),
        start_time: std::time::Instant::now(),
        origin_counters: Mutex::new(HashMap::new()),
        acme_challenges: Mutex::new(HashMap::new()),
    });
    let tls_key: String;
    let tls_chain: String;
    // print if JWT_SECRET is set in env
    if let Ok(_jwt_secret) = std::env::var("JWT_SECRET") {
        println!("Using JWT secret from environment variable");
    } else {
        println!("Using JWT secret from command line argument");
    }

    // Optional built-in ACME (Let's Encrypt) certificate management.
    // Enabled when KCIDB_DOMAINS is set (comma-separated list of domains).
    // CERTBOT_EMAIL is reused as the ACME account contact (defaults to
    // bot@kernelci.org). Set ACME_STAGING=1 to use the Let's Encrypt staging
    // environment while testing.
    let acme_domains = acme_domains_from_env();
    let acme_email = std::env::var("CERTBOT_EMAIL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bot@kernelci.org".to_string());
    let acme_staging = std::env::var("ACME_STAGING")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if let Some(domains) = &acme_domains {
        // Validate every domain syntactically before using it.
        for domain in domains {
            if !valid_hostname(domain) {
                eprintln!("Error: invalid domain in KCIDB_DOMAINS: {}", domain);
                std::process::exit(1);
            }
        }
        // Every domain must resolve in DNS, otherwise ACME HTTP-01 cannot work.
        for domain in domains {
            if !domain_resolves(domain).await {
                eprintln!(
                    "Error: domain '{}' from KCIDB_DOMAINS does not resolve in DNS",
                    domain
                );
                std::process::exit(1);
            }
        }
        let primary = &domains[0];
        // Make sure the certbot-style certificate directory exists and is
        // only accessible by the owner (mode 0700) before we use it.
        if let Err(e) = ensure_cert_dir(primary) {
            eprintln!("Error: failed to prepare certificate directory: {}", e);
            std::process::exit(1);
        }
        tls_key = format!("/etc/letsencrypt/live/{}/privkey.pem", primary);
        tls_chain = format!("/etc/letsencrypt/live/{}/fullchain.pem", primary);
        println!(
            "ACME: built-in certificate management enabled for {:?}",
            domains
        );
        if std::env::var("CERTBOT_DOMAIN").is_ok() {
            println!("ACME: KCIDB_DOMAINS is set, ignoring CERTBOT_DOMAIN");
        }
        // The HTTP-01 challenge server on port 80 must be up before issuance.
        // Bind synchronously here so a failure (e.g. the container lacks
        // CAP_NET_BIND_SERVICE / is not root) is fatal, instead of being
        // logged from a background task while we drop into an issuance retry
        // loop that can never succeed.
        let challenge_listener = match TcpListener::bind((args.host.as_str(), 80u16)).await {
            Ok(listener) => {
                println!("ACME: HTTP-01 challenge server listening on {}:80", args.host);
                listener
            }
            Err(e) => {
                eprintln!(
                    "Error: failed to bind ACME challenge server on {}:80: {}",
                    args.host, e
                );
                eprintln!("ACME: certificate issuance/renewal needs port 80 reachable");
                std::process::exit(1);
            }
        };
        tokio::spawn(run_challenge_server(
            challenge_listener,
            app_state.clone(),
            primary.clone(),
        ));
        // The TLS config built below needs a certificate on disk. If we do not
        // have a valid one, request it now and keep retrying every 30 minutes
        // (spaced out to stay within Let's Encrypt rate limits) until it works.
        if cert_needs_renewal(&tls_chain).await {
            println!("ACME: no valid certificate found, requesting one");
            let mut attempt = 0;
            loop {
                attempt += 1;
                match obtain_certificate(domains, &acme_email, acme_staging, &app_state).await {
                    Ok(()) => break,
                    Err(e) => {
                        eprintln!("ACME: issuance attempt {} failed: {}", attempt, e);
                        eprintln!("ACME: retrying in 60 minutes");
                        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
                    }
                }
            }
        } else {
            println!("ACME: existing certificate is still valid");
        }
    } else if let Ok(certbot_domain) = std::env::var("CERTBOT_DOMAIN") {
        // TODO: Remove this deprecated feature on next code cleanup.
        // External certbot: certificates are in /etc/letsencrypt/live/${CERTBOT_DOMAIN}/
        // fullchain.pem and privkey.pem
        tls_key = format!("/etc/letsencrypt/live/{}/privkey.pem", certbot_domain);
        tls_chain = format!("/etc/letsencrypt/live/{}/fullchain.pem", certbot_domain);
        // check if the file exists
        if wait_for_file(&tls_key) {
            println!(
                "Using TLS key from /etc/letsencrypt/live/{}/privkey.pem",
                certbot_domain
            );
        } else {
            eprintln!("Error: TLS key file {} does not exist", tls_key);
            std::process::exit(1);
        }
    } else {
        tls_key = String::new();
        tls_chain = String::new();
    }
    if !verify_submission_path(&app_state.directory) {
        eprintln!(
            "Error: submissions path {} does not exist or is not a directory",
            app_state.directory
        );
        std::process::exit(1);
    }
    // if default value - warn
    if app_state.jwt_secret == "secret" {
        eprintln!("Warning: JWT secret is default value");
    }
    // if secret is empty, warn
    if app_state.jwt_secret.is_empty() {
        eprintln!("Warning: JWT secret is empty, disabling authentication");
    }
    if !app_state.unified_secret.is_empty() {
        println!("Unified secret is configured");
    }
    let mut port = args.port;

    // if we are not root, change if port < 1024 to 8080
    if port < 1024 && !are_we_root() {
        println!("Warning: Port {} is less than 1024, you dont have root, using 8080 instead", args.port);
        port = 8080;
    }

    if tls_key.is_empty() && port == 0 {
        port = 80;
    } else if port == 0 {
        port = 443;
    }

    println!(
        "Listening on {}:{}, submissions path: {}",
        args.host, port, app_state.directory
    );

    // plain http if tls_key is empty
    if tls_key.is_empty() {
        println!("Starting HTTP server");
        let app = Router::new()
            .route("/", get(handle_root))
            .route("/submit", post(receive_submission))
            .route("/status", get(submission_status))
            .route("/metrics", get(submission_metrics))
            .route("/health", get(|| async { "OK" }))
            .route("/authtest", get(auth_test))
            .route("/.well-known/acme-challenge/{token}", get(serve_acme_challenge))
            .with_state(app_state)
            .layer(limit_layer)
            .layer(axum::extract::DefaultBodyLimit::max(512 * 1024 * 1024));
        let tcp_listener = match TcpListener::bind((args.host.as_str(), port)).await {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("Failed to bind to {}:{}: {}", args.host, port, e);
                std::process::exit(1);
            }
        };
        if let Err(e) = axum::serve(tcp_listener, app).await {
            eprintln!("HTTP server failed: {}", e);
            std::process::exit(1);
        }
    } else {
        println!(
            "Starting HTTPS server with TLS key: {} and chain: {}",
            tls_key, tls_chain
        );
        let app = Router::new()
            .route("/", get(handle_root))
            .route("/submit", post(receive_submission))
            .route("/status", get(submission_status))
            .route("/metrics", get(submission_metrics))
            .route("/health", get(|| async { "OK" }))
            .route("/authtest", get(auth_test))
            .route("/.well-known/acme-challenge/{token}", get(serve_acme_challenge))
            .with_state(app_state.clone())
            .layer(limit_layer)
            .layer(axum::extract::DefaultBodyLimit::max(512 * 1024 * 1024));
        //let tcp_listener = TcpListener::bind((args.host, args.port)).await.unwrap();
        let tls_config = match RustlsConfig::from_pem_file(&tls_chain, &tls_key).await {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to load TLS configuration: {}", e);
                std::process::exit(1);
            }
        };
        // Background ACME renewal: re-issue before expiry and hot-reload the
        // running TLS config without restarting the server.
        if let Some(domains) = acme_domains.clone() {
            let reload_config = tls_config.clone();
            let renew_state = app_state.clone();
            let chain_path = tls_chain.clone();
            let key_path = tls_key.clone();
            let email = acme_email.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
                    if !cert_needs_renewal(&chain_path).await {
                        continue;
                    }
                    println!("ACME: certificate nearing expiry, renewing");
                    match obtain_certificate(&domains, &email, acme_staging, &renew_state).await {
                        Ok(()) => match reload_config
                            .reload_from_pem_file(&chain_path, &key_path)
                            .await
                        {
                            Ok(()) => println!("ACME: renewed certificate reloaded"),
                            Err(e) => eprintln!("ACME: failed to reload certificate: {}", e),
                        },
                        Err(e) => eprintln!("ACME: renewal failed: {}", e),
                    }
                }
            });
        }
        let address = format!("{}:{}", args.host, port);
        let addr = match address.parse::<std::net::SocketAddr>() {
            Ok(addr) => addr,
            Err(e) => {
                eprintln!("Failed to parse address {}: {}", address, e);
                std::process::exit(1);
            }
        };
        if let Err(e) = axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service())
            .await {
            eprintln!("HTTPS server failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn verify_auth(headers: HeaderMap, state: Arc<AppState>) -> Result<JWT, String> {
    if state.jwt_secret.is_empty() {
        // If no secret, return a default JWT or handle as needed
        return Ok(JWT {
            origin: "none".to_string(),
        });
    }
    let jwt_r = headers.get("Authorization");
    let jwt = match jwt_r {
        Some(jwt) => jwt,
        None => return Err("JWT is required".to_string()),
    };
    let jwt_str_r = jwt.to_str();
    let jwt_str = match jwt_str_r {
        Ok(jwt_str) => jwt_str,
        Err(_) => return Err("Missing or invalid JWT".to_string()),
    };
    let jwt_str_r = jwt_str.split(" ").nth(1);
    let jwt_str = match jwt_str_r {
        Some(jwt_str) => jwt_str,
        None => return Err("Missing or invalid JWT (Bearer)".to_string()),
    };
    let jwt = verify_jwt(jwt_str, &state.jwt_secret);
    match jwt {
        Ok(jwt) => Ok(jwt),
        Err(e) => {
            if !state.unified_secret.is_empty() {
                let jwt2 = verify_jwt(jwt_str, &state.unified_secret);
                match jwt2 {
                    Ok(jwt) => Ok(jwt),
                    Err(_) => Err(e.to_string()),
                }
            } else {
                Err(e.to_string())
            }
        }
    }
}

fn generate_answer(status: &str, id: &str, message: Option<String>) -> String {
    let status = SubmissionStatus {
        id: id.to_string(),
        status: status.to_string(),
        message,
    };
    // serialize to json
    serde_json::to_string(&status).unwrap().to_string()
}


#[derive(serde::Deserialize)]
struct StatusQuery {
    id: String,
}
/*
/status?id=1234
*/
async fn submission_status(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<StatusQuery>,
) -> impl IntoResponse {
    let auth_result = verify_auth(headers, state.clone());
    match auth_result {
        Ok(jwt) => {
            println!("Authentication successful for origin: {}", jwt.origin);
        }
        Err(e) => {
            println!("Error: {}", e);
            state.user_error_counter.fetch_add(1, Ordering::Relaxed);
            let jsanswer = generate_answer("error", "0", Some(e));
            return (StatusCode::UNAUTHORIZED, jsanswer);
        }
    }
    let id = query.id;
    // validate id for safe characters
    if id.is_empty() {
        state.user_error_counter.fetch_add(1, Ordering::Relaxed);
        let jsanswer = generate_answer("error", "0", Some("Empty id".to_string()));
        return (StatusCode::BAD_REQUEST, jsanswer);
    }

    // id is alphanumeric
    if !id.chars().all(|c| c.is_alphanumeric()) {
        state.user_error_counter.fetch_add(1, Ordering::Relaxed);
        let jsanswer = generate_answer("error", "0", Some("Invalid id".to_string()));
        return (StatusCode::BAD_REQUEST, jsanswer);
    }

    let mut submission_file = format!("{}/submission-{}.json.temp", state.directory, id);
    // check if the file exists
    if Path::new(&submission_file).exists() {
        // check if the file is empty
        let jsanswer = generate_answer("inprogress", id.as_str(), Some("File still in progress".to_string()));
        return (StatusCode::OK, jsanswer)
    }

    submission_file = format!("{}/submission-{}.json", state.directory, id);
    // check if the submission file exists
    if Path::new(&submission_file).exists() {
        let jsanswer = generate_answer("ready", id.as_str(), Some("File waiting for processing".to_string()));
        return (StatusCode::OK, jsanswer);
    }

    submission_file = format!("{}/archive/submission-{}.json", state.directory, id);
    // check if the archived file exists
    if Path::new(&submission_file).exists() {
        let jsanswer = generate_answer("processed", id.as_str(), Some("File archived".to_string()));
        return (StatusCode::OK, jsanswer);
    }

    submission_file = format!("{}/failed/submission-{}.json", state.directory, id);
    // check if the failed file exists
    if Path::new(&submission_file).exists() {
        let jsanswer = generate_answer("failed", id.as_str(), Some("File failed to pass validation".to_string()));
        return (StatusCode::OK, jsanswer);
    }

    state.user_error_counter.fetch_add(1, Ordering::Relaxed);
    let jsanswer = generate_answer("notfound", id.as_str(), Some("File not found".to_string()));
    
    return (StatusCode::NOT_FOUND, jsanswer);
}

// Answer STATUS 200 if the submission is valid
async fn receive_submission(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    let auth_result = verify_auth(headers, state.clone());
    let origin: String;
    match auth_result {
        Ok(jwt) => {
            origin = jwt.origin;
        }
        Err(e) => {
            println!("Error: {}", e);
            let err_status = SubmissionStatus {
                id: "0".to_string(),
                status: "error".to_string(),
                message: Some(e),
            };
            let err_json = serde_json::to_string(&err_status).unwrap();
            // increment error counter atomically
            state.error_counter.fetch_add(1, Ordering::Relaxed);

            return (StatusCode::UNAUTHORIZED, err_json);
        }
    }

    let submission_json = serde_json::from_str::<serde_json::Value>(&body);
    match submission_json {
        Ok(_submission) => {
            let size = body.len();
            println!("Received submission size: {} from {} ", size, origin);
            let submission_id = random_string(32);
            let submission_file =
                format!("{}/submission-{}.json.temp", state.directory, submission_id);
            if let Err(e) = std::fs::write(&submission_file, &body) {
                eprintln!("Failed to write submission file: {}", e);
                state.system_error_counter.fetch_add(1, Ordering::Relaxed);
                let err_status = SubmissionStatus {
                    id: "0".to_string(),
                    status: "error".to_string(),
                    message: Some("Internal server error".to_string()),
                };
                let err_json = serde_json::to_string(&err_status).unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, err_json);
            }
            // on completion, rename to submission.json
            if let Err(e) = std::fs::rename(
                &submission_file,
                format!("{}/submission-{}.json", state.directory, submission_id),
            ) {
                eprintln!("Failed to rename submission file: {}", e);
                state.system_error_counter.fetch_add(1, Ordering::Relaxed);
                let err_status = SubmissionStatus {
                    id: "0".to_string(),
                    status: "error".to_string(),
                    message: Some("Internal server error".to_string()),
                };
                let err_json = serde_json::to_string(&err_status).unwrap_or_default();
                return (StatusCode::INTERNAL_SERVER_ERROR, err_json);
            }
            println!("Submission {} received", submission_id);
            let msg = format!(
                "Received submission {} with size {} bytes",
                submission_id, size
            );

            let status = SubmissionStatus {
                id: submission_id,
                status: "ok".to_string(),
                message: Some(msg),
            };
            let jsonstr = serde_json::to_string(&status).unwrap();
            // increment submission counter atomically
            state.submission_counter.fetch_add(1, Ordering::Relaxed);
            state
                .submission_size_total
                .fetch_add(size as u64, Ordering::Relaxed);
            // increment per-origin counter
            let normalized = normalize_origin(&origin);
            if let Ok(mut counters) = state.origin_counters.lock() {
                *counters.entry(normalized).or_insert(0) += 1;
            }
            println!("Submission status: {}", jsonstr);
            (StatusCode::OK, jsonstr)
        }
        Err(e) => {
            println!("Error: {}", e);
            state.user_error_counter.fetch_add(1, Ordering::Relaxed);
            let err_status = SubmissionStatus {
                id: "0".to_string(),
                status: "error".to_string(),
                message: Some(e.to_string()),
            };
            let err_json = serde_json::to_string(&err_status).unwrap();
            (StatusCode::BAD_REQUEST, err_json)
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JWT {
    origin: String,
}

fn verify_jwt(token: &str, secret: &str) -> Result<JWT, jsonwebtoken::errors::Error> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::default();
    validation.validate_aud = false;
    let token = decode::<JWT>(token, &key, &validation)?;
    Ok(token.claims)
}

/* STUB for now */
/*
fn generate_jwt(origin: &str, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let key = EncodingKey::from_secret(secret.as_bytes());
    let token = encode(&Header::default(), &JWT { origin: origin.to_string() }, &key)?;
    Ok(token)
}
*/

// TODO: Fix this
fn random_string(length: usize) -> String {
    let mut rng = rand::rng();
    // rng.sample(rand::distr::Alphanumeric) as char
    let mut s = String::new();
    for _ in 0..length {
        s.push(rng.sample(rand::distr::Alphanumeric) as char);
    }
    s
}

// ----------------------------------------------------------------------------
// Optional built-in ACME (Let's Encrypt) certificate management.
// Active only when the KCIDB_DOMAINS environment variable is set.
// ----------------------------------------------------------------------------

type AcmeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Parse the optional KCIDB_DOMAINS environment variable into a list of domains.
/// Returns None when unset or empty, which disables built-in ACME.
fn acme_domains_from_env() -> Option<Vec<String>> {
    let raw = std::env::var("KCIDB_DOMAINS").ok()?;
    let domains: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if domains.is_empty() {
        None
    } else {
        Some(domains)
    }
}

/// Minimal hostname validation for domains handed to the ACME client.
fn valid_hostname(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.starts_with('-')
        && !domain.ends_with('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Check that `domain` resolves to at least one IP address via DNS.
/// ACME HTTP-01 validation cannot succeed for a domain that does not resolve.
async fn domain_resolves(domain: &str) -> bool {
    match tokio::net::lookup_host((domain, 0u16)).await {
        Ok(mut addrs) => addrs.next().is_some(),
        Err(_) => false,
    }
}

/// Ensure the certbot-style certificate directory for `primary` exists and is
/// only accessible by the owner (mode 0700), since private keys live there.
/// Returns the path of the live directory.
fn ensure_cert_dir(primary: &str) -> std::io::Result<String> {
    let live_dir = format!("/etc/letsencrypt/live/{}", primary);
    if !Path::new(&live_dir).exists() {
        std::fs::create_dir_all(&live_dir)?;
        println!("ACME: created certificate directory {}", live_dir);
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort on the mount root; strict on the directories we manage.
        let _ = std::fs::set_permissions(
            "/etc/letsencrypt",
            std::fs::Permissions::from_mode(0o700),
        );
        for dir in ["/etc/letsencrypt/live", live_dir.as_str()] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(live_dir)
}

/// Returns true if the certificate at `cert_path` is missing, unreadable,
/// already expired, or expires within the next 30 days.
async fn cert_needs_renewal(cert_path: &str) -> bool {
    let data = match tokio::fs::read(cert_path).await {
        Ok(data) => data,
        Err(_) => return true,
    };
    match x509_parser::pem::parse_x509_pem(&data) {
        Ok((_, pem)) => match pem.parse_x509() {
            Ok(cert) => match cert.validity().time_to_expiration() {
                Some(remaining) => remaining.whole_days() < 30,
                None => true,
            },
            Err(_) => true,
        },
        Err(_) => true,
    }
}

/// Serve the ACME HTTP-01 challenge endpoint on port 80 and redirect every
/// other request to HTTPS. Runs as a background task when built-in ACME is on.
/// The port-80 `listener` is bound by the caller so that a bind failure is a
/// hard startup error rather than a silently logged background-task failure.
/// `canonical_domain` is the primary KCIDB_DOMAINS entry; HTTPS redirects are
/// built from it rather than the request's Host header.
async fn run_challenge_server(
    listener: TcpListener,
    state: Arc<AppState>,
    canonical_domain: String,
) {
    let app = Router::new()
        .route(
            "/.well-known/acme-challenge/{token}",
            get(serve_acme_challenge),
        )
        .route("/health", get(|| async { "OK" }))
        .fallback(move |uri: axum::http::Uri| {
            redirect_to_https(canonical_domain.clone(), uri)
        })
        .with_state(state);
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("ACME: challenge server stopped: {}", e);
    }
}

/// Redirect a plain HTTP request to its HTTPS equivalent.
///
/// The Location is built from `canonical_domain` (the configured primary
/// domain), not the request's Host header. The Host header is attacker-
/// controlled, so trusting it would allow open-redirect abuse and can also
/// produce malformed targets such as `https://example.com:80/...` when the
/// client sends an explicit `:80` port.
async fn redirect_to_https(canonical_domain: String, uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    axum::response::Redirect::permanent(&format!("https://{}{}", canonical_domain, path))
}

/// Ensure /etc/letsencrypt exists and is owner-only, since both the ACME
/// account credentials and the certificate private keys are stored beneath it.
fn ensure_letsencrypt_dir() -> std::io::Result<()> {
    let root = "/etc/letsencrypt";
    if !Path::new(root).exists() {
        std::fs::create_dir_all(root)?;
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort: the mount root may be owned by another uid.
        let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Atomically write `contents` to `path` with permission `mode`.
///
/// Certificate and key material must never be observed half-written: a
/// concurrent TLS reload (or any external reader sharing the volume mount)
/// that picks up a truncated PEM gets an unusable file. A plain
/// `tokio::fs::write` truncates the target up front and streams bytes into it,
/// so both a crash mid-write and a concurrent read expose a partial file.
///
/// Instead we write into a uniquely named temp file in the *same directory*
/// (so the final `rename` stays on one filesystem and is therefore atomic),
/// fsync it so the bytes are durable, set `mode` *before* the rename so the
/// target never appears with looser permissions, then rename it into place.
/// rename(2) is atomic, so a reader of `path` always sees either the complete
/// previous file or the complete new one. A crash leaves only a stale temp
/// file; `path` keeps its previous valid contents (or stays absent).
async fn atomic_write(path: &str, contents: &[u8], mode: u32) -> std::io::Result<()> {
    let target = Path::new(path);
    let dir = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("atomic_write: path has no parent directory: {}", path),
        )
    })?;
    let file_name = target.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("atomic_write: path has no file name: {}", path),
        )
    })?;

    // Temp name is dotted (hidden) and randomized so a crash leaves something
    // obviously transient behind and concurrent writers cannot collide.
    let tmp_path = dir.join(format!(
        ".{}.tmp.{}",
        file_name.to_string_lossy(),
        random_string(12)
    ));

    // Write + flush + fsync the temp file. On any failure, remove the temp
    // file so we never accumulate stale fragments next to the real cert.
    let write_result = async {
        let mut file = tokio::fs::File::create(&tmp_path).await?;
        file.write_all(contents).await?;
        file.flush().await?;
        // Force the bytes to disk before the rename, otherwise a crash could
        // leave the rename committed but the file's contents still empty.
        file.sync_all().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    // Apply the requested mode while the file still has its temporary name,
    // so `path` is never visible with default (umask-derived) permissions.
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode)).await
        {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
    }
    #[cfg(not(target_family = "unix"))]
    let _ = mode;

    // The atomic swap. On failure, drop the temp file rather than leak it.
    if let Err(e) = tokio::fs::rename(&tmp_path, target).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    // Best-effort: fsync the directory so the rename itself survives a crash.
    // Not all platforms allow opening a directory as a file; ignore failures.
    if let Ok(dir_handle) = tokio::fs::File::open(dir).await {
        let _ = dir_handle.sync_all().await;
    }

    Ok(())
}

/// Path of the persisted ACME account credentials. Kept inside /etc/letsencrypt
/// so it survives container restarts via the same volume mount as the certs.
/// Staging and production accounts live on different ACME servers and on
/// distinct key pairs, so each environment gets its own file.
fn acme_account_path(staging: bool) -> &'static str {
    if staging {
        "/etc/letsencrypt/acme-account-staging.json"
    } else {
        "/etc/letsencrypt/acme-account.json"
    }
}

/// Restore a previously persisted ACME account, or register a fresh one and
/// persist its credentials. Reusing the account across issuances and renewals
/// avoids registering a new account every time, which would churn accounts and
/// can trip ACME provider rate limits.
///
/// instant-acme's `Account::builder()` sets up its own HTTPS client and crypto
/// provider; both `create` and `from_credentials` go through the same builder.
async fn load_or_create_account(
    email: &str,
    staging: bool,
    directory_url: &str,
) -> AcmeResult<Account> {
    let path = acme_account_path(staging);

    // Try to restore an existing account from persisted credentials first.
    match tokio::fs::read(path).await {
        Ok(data) => match serde_json::from_slice::<AccountCredentials>(&data) {
            Ok(credentials) => match Account::builder()?.from_credentials(credentials).await {
                Ok(account) => {
                    println!("ACME: reusing persisted account credentials from {}", path);
                    return Ok(account);
                }
                Err(e) => eprintln!(
                    "ACME: stored account in {} is unusable ({}); registering a new account",
                    path, e
                ),
            },
            Err(e) => eprintln!(
                "ACME: could not parse account credentials in {} ({}); registering a new account",
                path, e
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("ACME: no persisted account at {}; registering a new account", path);
        }
        Err(e) => eprintln!(
            "ACME: could not read account credentials at {} ({}); registering a new account",
            path, e
        ),
    }

    // No usable credentials: register a new account and persist it.
    let contacts = [format!("mailto:{}", email)];
    let contact_refs: Vec<&str> = contacts.iter().map(String::as_str).collect();
    let (account, credentials) = Account::builder()?
        .create(
            &NewAccount {
                contact: &contact_refs,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory_url.to_owned(),
            None,
        )
        .await?;

    // Persist the credentials so future renewals reuse this account. Failure
    // here is non-fatal: issuance can still proceed with the in-memory account.
    match serde_json::to_vec(&credentials) {
        Ok(serialized) => {
            if let Err(e) = ensure_letsencrypt_dir() {
                eprintln!("ACME: could not prepare {} ({}); account not persisted",
                    "/etc/letsencrypt", e);
            } else if let Err(e) = atomic_write(path, &serialized, 0o600).await {
                // The file holds the account private key, so it is written
                // atomically with 0600 permissions already in place.
                eprintln!("ACME: failed to persist account credentials to {}: {}", path, e);
            } else {
                println!("ACME: registered new account, credentials saved to {}", path);
            }
        }
        Err(e) => eprintln!("ACME: failed to serialize account credentials: {}", e),
    }

    Ok(account)
}

/// Obtain (or renew) a certificate for `domains` via the ACME HTTP-01 flow and
/// write it into /etc/letsencrypt/live/<primary-domain>/ in the certbot layout.
async fn obtain_certificate(
    domains: &[String],
    email: &str,
    staging: bool,
    state: &Arc<AppState>,
) -> AcmeResult<()> {
    let directory_url = if staging {
        LetsEncrypt::Staging.url()
    } else {
        LetsEncrypt::Production.url()
    };
    println!(
        "ACME: requesting certificate for {:?} ({} environment)",
        domains,
        if staging { "staging" } else { "production" }
    );

    // Reuse a persisted ACME account if one exists, otherwise register a new
    // one and persist it. This keeps a single stable account across renewals
    // instead of churning a new account on every issuance.
    let account = load_or_create_account(email, staging, directory_url).await?;

    // Create the order covering every requested domain.
    let identifiers: Vec<Identifier> =
        domains.iter().map(|d| Identifier::Dns(d.clone())).collect();
    let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;

    // Publish an HTTP-01 response for each pending authorization.
    let mut tokens: Vec<String> = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(result) = authorizations.next().await {
        let mut authz = result?;
        match authz.status {
            AuthorizationStatus::Pending => {}
            AuthorizationStatus::Valid => continue,
            ref other => {
                return Err(format!("unexpected authorization status: {:?}", other).into());
            }
        }
        let mut challenge = authz
            .challenge(ChallengeType::Http01)
            .ok_or("ACME server did not offer an HTTP-01 challenge")?;
        let token = challenge.token.clone();
        let key_authorization = challenge.key_authorization().as_str().to_string();
        if let Ok(mut map) = state.acme_challenges.lock() {
            map.insert(token.clone(), key_authorization);
        }
        tokens.push(token);
        challenge.set_ready().await?;
    }

    // Wait for validation, then finalize and download the certificate.
    // (`authorizations` borrows `order`; its borrow ends with the loop above.)
    let outcome = async {
        let status = order.poll_ready(&RetryPolicy::default()).await?;
        if status != OrderStatus::Ready {
            return Err(format!("ACME order did not become ready: {:?}", status).into());
        }
        let private_key_pem = order.finalize().await?;
        let cert_chain_pem = order.poll_certificate(&RetryPolicy::default()).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((private_key_pem, cert_chain_pem))
    }
    .await;

    // Drop the published challenge responses whether or not issuance succeeded.
    if let Ok(mut map) = state.acme_challenges.lock() {
        for token in &tokens {
            map.remove(token);
        }
    }

    let (private_key_pem, cert_chain_pem) = outcome?;

    // Persist in the certbot-compatible layout so existing volume mounts work.
    let live_dir = ensure_cert_dir(&domains[0])?;
    let key_path = format!("{}/privkey.pem", live_dir);
    let chain_path = format!("{}/fullchain.pem", live_dir);
    // Write both files atomically so a concurrent TLS reload never reads a
    // half-written PEM. privkey.pem holds the private key (owner-only, 0600);
    // fullchain.pem is public material but uses the same atomic path.
    //
    // TODO: each file is individually atomic, but the *pair* is not. A reader
    // that runs between these two writes can pick up the new privkey.pem with
    // the old fullchain.pem (or vice versa). Within this process that is safe
    // (reload_from_pem_file runs only after both writes return), but an
    // external consumer sharing the /etc/letsencrypt volume could catch the
    // gap. To make the pair atomic, write into a fresh live/<domain>.<random>/
    // directory and rename the directory into place (or symlink-swap it).
    atomic_write(&key_path, private_key_pem.as_bytes(), 0o600).await?;
    atomic_write(&chain_path, cert_chain_pem.as_bytes(), 0o644).await?;
    println!("ACME: certificate stored in {}", live_dir);
    Ok(())
}
