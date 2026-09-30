//! reqwest 시나리오 — 리터럴·format!·concat!·+·Url::parse/join·지역 변수·static.

use crate::{Res, HOST};
use reqwest::blocking::Client;
use std::sync::LazyLock;
use url::Url;

/// 경로가 있는 base.
const V1: &str = "http://api.example.com/v1";

/// 끝 슬래시가 있는 base — 문자열 연결은 `//`를 남긴다.
const V3_SLASH: &str = "http://api.example.com/v3/";

/// 전역 클라이언트.
static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

pub fn literal_query() -> Res {
    reqwest::blocking::get("http://api.example.com/v1/items?page=2")?;
    Ok(())
}

pub async fn async_user(id: u64) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();
    client.get(format!("{V1}/users/{id}")).send().await?;
    Ok(())
}

pub fn concat_post() -> Res {
    Client::new()
        .post(concat!("http://api.example.com", "/v1/orders"))
        .send()?;
    Ok(())
}

pub fn put_positional(id: u32) -> Res {
    let client = Client::new();
    client.put(format!("{}/v1/items/{}", HOST, id)).send()?;
    Ok(())
}

pub fn delete_plus(id: &str) -> Res {
    CLIENT
        .delete(HOST.to_string() + "/v1/items/" + id)
        .send()?;
    Ok(())
}

pub fn patch_local() -> Res {
    let url = format!("{HOST}/v1/profile");
    CLIENT.patch(&url).send()?;
    Ok(())
}

pub fn head_health() -> Res {
    CLIENT.head(format!("{V1}/health")).send()?;
    Ok(())
}

pub fn request_options() -> Res {
    CLIENT
        .request(reqwest::Method::OPTIONS, format!("{V1}/items"))
        .send()?;
    Ok(())
}

pub fn request_dynamic(method: reqwest::Method) -> Res {
    CLIENT.request(method, format!("{V1}/verbs")).send()?;
    Ok(())
}

pub fn join_relative(id: u32) -> Res {
    let base = Url::parse("http://api.example.com/v2/")?;
    CLIENT.get(base.join(&format!("users/{id}"))?).send()?;
    Ok(())
}

pub fn join_replaces_last() -> Res {
    let url = Url::parse("http://api.example.com/v2/catalog")?.join("tags")?;
    CLIENT.get(url).send()?;
    Ok(())
}

pub fn join_absolute_path() -> Res {
    let url = Url::parse(V1)?.join("/root/ping")?;
    CLIENT.get(url.as_str()).send()?;
    Ok(())
}

pub fn dot_segments() -> Res {
    CLIENT.get("http://api.example.com/v1/a/../b").send()?;
    Ok(())
}

pub fn double_slash() -> Res {
    CLIENT.get(format!("{V3_SLASH}/items")).send()?;
    Ok(())
}

pub fn query_tail(page: Option<u32>) -> Res {
    let q = if let Some(p) = page {
        format!("?page={p}")
    } else {
        String::new()
    };
    CLIENT.get(format!("{V1}/search{q}")).send()?;
    Ok(())
}

pub fn partial_segment(name: &str) -> Res {
    CLIENT.get(format!("{V1}/files/{name}.json")).send()?;
    Ok(())
}

pub fn unknown_base_rooted(base: &str) -> Res {
    CLIENT.get(format!("{base}/v1/status")).send()?;
    Ok(())
}

pub fn unknown_base_glued(base: &str) -> Res {
    CLIENT.get(format!("{base}status")).send()?;
    Ok(())
}

pub fn relative_url() -> Res {
    CLIENT.get("/v1/relative").send()?;
    Ok(())
}

pub fn masked_token() -> Res {
    CLIENT
        .get(format!("{V1}/tokens/a1b2c3d4e5f6a7b8c9d0"))
        .send()?;
    Ok(())
}

pub fn non_ascii() -> Res {
    CLIENT.get("http://api.example.com/v1/café").send()?;
    Ok(())
}

pub fn userinfo_fragment() -> Res {
    CLIENT
        .get("http://user:pw@API.example.com/v1/secure#top")
        .send()?;
    Ok(())
}

pub fn with_port() -> Res {
    CLIENT.get("http://api.example.com:8080/v1/port").send()?;
    Ok(())
}

pub fn backslashes() -> Res {
    CLIENT.get("http://api.example.com\\v1\\bs").send()?;
    Ok(())
}

pub fn builder_client() -> Res {
    let client = reqwest::blocking::Client::builder()
        .user_agent("fixture")
        .build()?;
    client.get(format!("{V1}/built")).send()?;
    Ok(())
}

pub fn struct_field_base() -> Res {
    let api = crate::api::ApiClient::new();
    api.list_items()?;
    Ok(())
}

pub fn struct_field_item() -> Res {
    crate::api::ApiClient::new().get_item(42)
}

pub fn struct_field_plus() -> Res {
    crate::api::ApiClient::new().create_item()
}

pub fn struct_field_query() -> Res {
    crate::api::ApiClient::new().search("x")
}

pub fn struct_param_base(base: &str) -> Res {
    crate::api::RemoteClient::new(base).user(3)
}

pub fn url_field() -> Res {
    crate::api::Catalog::new().products()
}
