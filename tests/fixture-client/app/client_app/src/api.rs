//! 구조체 필드에 base URL을 두는 API 클라이언트 — e2e에서 axum08 fixture 서버의
//! `/api/items` 라우트를 부른다.

use crate::Res;
use reqwest::blocking::Client;
use url::Url;

/// 생성자가 base를 상수로 채운다 — 필드 값이 리터럴로 풀린다.
pub struct ApiClient {
    base_url: String,
    http: Client,
}

/// API base(경로 포함).
const API_BASE: &str = "http://api.example.com/api";

impl ApiClient {
    pub fn new() -> Self {
        Self {
            base_url: API_BASE.to_string(),
            http: Client::new(),
        }
    }

    pub fn list_items(&self) -> Res {
        self.http.get(format!("{}/items", self.base_url)).send()?;
        Ok(())
    }

    pub fn get_item(&self, id: u64) -> Res {
        self.http
            .get(format!("{}/items/{id}", self.base_url))
            .send()?;
        Ok(())
    }

    pub fn create_item(&self) -> Res {
        self.http.post(self.base_url.clone() + "/items").send()?;
        Ok(())
    }

    pub fn search(&self, q: &str) -> Res {
        self.http
            .get(format!("{}/search?q={q}", self.base_url))
            .send()?;
        Ok(())
    }

    /// 선언된 래퍼(`http-wrappers.json`) — 동사는 enum, 경로는 base 뒤다. 본문의 동적
    /// 호출은 래퍼 호출 사실이 대신한다.
    pub fn send(&self, verb: Verb, path: &str) -> Res {
        let method = match verb {
            Verb::Get => reqwest::Method::GET,
            Verb::Post => reqwest::Method::POST,
            Verb::Delete => reqwest::Method::DELETE,
        };
        self.http
            .request(method, format!("{}{}", self.base_url, path))
            .send()?;
        Ok(())
    }

    /// 구조체 리터럴 생성 래퍼 `Endpoint`를 실행한다.
    pub fn execute(&self, ep: Endpoint) -> Res {
        self.send(ep.verb, &ep.path)
    }
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

/// 래퍼 동사 enum.
pub enum Verb {
    Get,
    Post,
    Delete,
}

/// 생성자 래퍼(`kind: constructor`, 이름 = 타입 이름) — 필드 레이블로 묶는다.
pub struct Endpoint {
    pub verb: Verb,
    pub path: String,
}

/// 생성자가 base를 매개변수로 받는다 — 필드가 풀리지 않아 base 앵커와 baseRef다.
pub struct RemoteClient {
    base: String,
    http: Client,
}

impl RemoteClient {
    pub fn new(base: &str) -> Self {
        RemoteClient {
            base: base.to_string(),
            http: Client::new(),
        }
    }

    pub fn user(&self, id: u32) -> Res {
        self.http
            .get(format!("{}/users/{}", self.base, id))
            .send()?;
        Ok(())
    }
}

/// `url::Url` 필드 — `Url::join`(RFC 3986 병합)으로 잇는다.
pub struct Catalog {
    base: Url,
    http: Client,
}

impl Catalog {
    pub fn new() -> Self {
        Catalog {
            base: Url::parse("http://api.example.com/cat/").unwrap(),
            http: Client::new(),
        }
    }

    pub fn products(&self) -> Res {
        self.http.get(self.base.join("products")?).send()?;
        Ok(())
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}
