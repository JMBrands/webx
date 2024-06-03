mod secret;

use actix_extensible_rate_limit::{
    backend::{memory::InMemoryBackend, SimpleInputFunctionBuilder},
    RateLimiter,
};

use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use dotenv::dotenv;
use futures::stream::StreamExt;
use lazy_static::lazy_static;
use mongodb::{bson::doc, options::ClientOptions, Client, Collection};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Domain {
    tld: String,
    ip: String,
    name: String,
    secret_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResponseDomain {
    tld: String,
    ip: String,
    name: String,
}

lazy_static! {
    static ref TLD: Vec<&'static str> = vec!["mf", "btw", "fr", "yap", "dev", "scam", "zip", "root", "web", "rizz", "habibi", "sigma", "now", "it", "soy", "lol", "uwu", "ohio"];
    static ref OFFENSIVE_WORDS: Vec<&'static str> = vec!["nigg", "sex", "porn"];
    static ref DB: TokioMutex<Option<Collection<Domain>>> = TokioMutex::new(None);
}

async fn connect_to_mongo() {
    dotenv().ok();
    let mongo_uri = std::env::var("MONGOURI").expect("MONGOURI must be set");
    let mut client_options = ClientOptions::parse(&mongo_uri).await.unwrap();
    client_options.app_name = Some("DomainApp".to_string());
    let client = Client::with_options(client_options).unwrap();
    let db = client.database("mydb");
    let collection = db.collection::<Domain>("domains");

    let mut db_lock = DB.lock().await;
    *db_lock = Some(collection);

    println!("connected to mongodb");
}

#[actix_web::get("/")]
async fn index() -> impl Responder {
    HttpResponse::Ok().body(
        "Hello, world! The available endpoints are:\nGET /domains,\nGET /domain/{name}/{tld},\nPOST /domain,\nPUT /domain/{key},\nDELETE /domain/{key},\nGET /tlds.\nRatelimits provided in headers.\n",
    )
}

async fn create_domain(domain: web::Json<Domain>) -> impl Responder {
    let secret_key = secret::generate(31);
    let mut domain = domain.into_inner();
    domain.secret_key = Some(secret_key.clone());

    if !TLD.contains(&domain.tld.as_str()) || !domain.name.chars().all(|c| c.is_alphabetic() || c == '-') || domain.name.len() > 24 {
        return HttpResponse::BadRequest().body("Invalid name, non-existent TLD, or name too long (24 chars).");
    }

    if OFFENSIVE_WORDS.iter().any(|word| domain.name.contains(word)) {
        return HttpResponse::BadRequest().body("The given domain is offensive.");
    }

    let collection = DB.lock().await;
    let collection = collection.as_ref().unwrap();

    let existing_domain = collection.find_one(doc! { "name": &domain.name, "tld": &domain.tld }, None).await.unwrap();

    if existing_domain.is_some() {
        return HttpResponse::Conflict().finish();
    }

    let insert_result = collection.insert_one(domain.clone(), None).await;

    match insert_result {
        Ok(_) => HttpResponse::Ok().json(domain),
        Err(_) => HttpResponse::Conflict().finish(),
    }
}

#[actix_web::get("/domain/{name}/{tld}")]
async fn get_domain(path: web::Path<(String, String)>) -> impl Responder {
    let collection = DB.lock().await;
    let collection = collection.as_ref().unwrap();

    let (name, tld) = path.into_inner();
    let filter = doc! { "name": name, "tld": tld };

    match collection.find_one(filter, None).await {
        Ok(Some(domain)) => HttpResponse::Ok().json(ResponseDomain {
            tld: domain.tld,
            name: domain.name,
            ip: domain.ip,
        }),
        Ok(None) => HttpResponse::NotFound().finish(),
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

#[actix_web::put("/domain/{key}")]
async fn update_domain(path: web::Path<String>, domain_update: web::Json<Domain>) -> impl Responder {
    let collection = DB.lock().await;
    let collection = collection.as_ref().unwrap();

    let key = path.into_inner();
    let filter = doc! { "secret_key": key };
    let update = doc! { "$set": { "ip": &domain_update.ip } };

    match collection.update_one(filter, update, None).await {
        Ok(result) => {
            if result.matched_count == 1 {
                HttpResponse::Ok().json(domain_update.into_inner())
            } else {
                HttpResponse::NotFound().finish()
            }
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

#[actix_web::delete("/domain/{key}")]
async fn delete_domain(path: web::Path<String>) -> impl Responder {
    let collection = DB.lock().await;
    let collection = collection.as_ref().unwrap();

    let key = path.into_inner();
    let filter = doc! { "secret_key": key };

    match collection.delete_one(filter, None).await {
        Ok(result) => {
            if result.deleted_count == 1 {
                HttpResponse::Ok().finish()
            } else {
                HttpResponse::NotFound().finish()
            }
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

#[actix_web::get("/domains")]
async fn get_domains() -> impl Responder {
    let collection = DB.lock().await;
    let collection = collection.as_ref().unwrap();
    let cursor = collection.find(None, None).await.unwrap();

    let domains: Vec<ResponseDomain> = cursor
        .filter_map(|result| async {
            match result {
                Ok(domain) => Some(ResponseDomain {
                    tld: domain.tld,
                    name: domain.name,
                    ip: domain.ip,
                }),
                Err(_) => None,
            }
        })
        .collect()
        .await;

    HttpResponse::Ok().json(domains)
}

#[actix_web::get("/tlds")]
async fn get_tlds() -> impl Responder { HttpResponse::Ok().json(&*TLD) }

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let backend = InMemoryBackend::builder().build();

    connect_to_mongo().await;
    // migrate to logger crate
    println!("listening on 127.0.0.1:8000");

    // generate api keys and store in kv db, be more leinent with ratelimit on those users

    /* cli:
     server <start/generate-key/--config (config.toml:default)>
    */

    // maybe use other db formats like postgres for storing the data WIP

    HttpServer::new(move || {
        let input = SimpleInputFunctionBuilder::new(Duration::from_secs(60), 10).real_ip_key().build();
        let middleware = RateLimiter::builder(backend.clone(), input).add_headers().build();

        App::new()
            .service(index)
            .service(get_domain)
            .service(update_domain)
            .service(delete_domain)
            .service(get_domains)
            .service(get_tlds)
            .route("/domain", web::post().wrap(middleware).to(create_domain))
    })
    .bind("127.0.0.1:8000")?
    .run()
    .await
}
