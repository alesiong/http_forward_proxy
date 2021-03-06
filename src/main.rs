use std::convert::Infallible;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use clap::{App, Arg};
use hyper::{
    Body,
    Client,
    client::HttpConnector,
    Request,
    Response,
    Server,
    service::{make_service_fn, service_fn},
    StatusCode,
    Uri,
};
use hyper_proxy::{Intercept, Proxy, ProxyConnector};

struct Config {
    proxy_to: Uri,
    via_proxy: Option<ProxyConnector<HttpConnector>>,
}

async fn forward(req: Request<Body>, config: Arc<Config>) -> Result<Response<Body>, hyper::http::Error> {
    async fn handle(mut req: Request<Body>, config: Arc<Config>) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
        let uri = req.uri();
        let new_uri_builder = Uri::builder()
            .authority(
                config.proxy_to.authority()
                    .map(|a| a.as_str())
                    .unwrap_or("")
            )
            .path_and_query(
                uri.path_and_query()
                    .map(|p| p.as_str())
                    .unwrap_or("/")
            )
            .scheme(
                config.proxy_to.scheme_str().unwrap_or("http")
            );

        let new_uri = new_uri_builder.build()?;
        *req.uri_mut() = new_uri;

        let resp = match &config.via_proxy {
            Some(proxy) => {
                Client::builder().build(proxy.clone()).request(req).await?
            }
            None => Client::new().request(req).await?,
        };

        println!("{}", resp.status());

        Ok(resp)
    }

    handle(req, config).await.or_else(|e| {
        eprintln!("error: {}", e);
        Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body("".into())
    })
}

#[tokio::main]
async fn main() {
    let matches = App::new("HTTP Forward Proxy")
        .arg(Arg::with_name("listen")
            .short("l")
            .long("listen")
            .default_value("127.0.0.1:9999")
            .help("listening address and port")
            .required(true)
            .takes_value(true)
        )
        .arg(Arg::with_name("to")
            .short("t")
            .long("to")
            .help("forward proxy to")
            .required(true)
            .takes_value(true)
        )
        .arg(Arg::with_name("via")
            .short("v")
            .long("via")
            .help("via http proxy")
            .required(false)
            .takes_value(true)
        )
        .get_matches();

    let addr = SocketAddr::from_str(matches.value_of("listen").unwrap()).unwrap();

    #[cfg(any(feature = "tls", feature = "rustls"))]
    fn get_proxy_connector(connector: HttpConnector, proxy: Proxy) -> ProxyConnector<HttpConnector> {
        ProxyConnector::from_proxy(connector, proxy).unwrap()
    }

    #[cfg(not(any(feature = "tls", feature = "rustls")))]
    fn get_proxy_connector(connector: HttpConnector, proxy: Proxy) -> ProxyConnector<HttpConnector> {
        ProxyConnector::from_proxy_unsecured(connector, proxy)
    }


    let config = Arc::new(Config {
        proxy_to: Uri::from_str(matches.value_of("to").unwrap()).unwrap(),
        via_proxy: {
            matches.value_of("via").and_then(|via| {
                via.parse::<Uri>().ok()
            }).map(|proxy_uri| {
                let proxy = Proxy::new(Intercept::All, proxy_uri);
                let connector = HttpConnector::new();
                get_proxy_connector(connector, proxy)
            })
        },
    });


    let make_svc = make_service_fn(|_conn| {
        let inner_config = Arc::clone(&config);
        async move {
            Ok::<_, Infallible>(service_fn(move |req| { forward(req, Arc::clone(&inner_config)) }))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}
