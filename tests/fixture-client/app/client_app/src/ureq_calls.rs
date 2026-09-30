//! ureq 3 시나리오 — 자유 함수와 Agent. ureq 3은 `http::Uri`로 해석해 점 세그먼트를
//! 지우지 않는다.

use crate::{Res, HOST};

pub fn ureq_get() -> Res {
    ureq::get("http://api.example.com/v1/ureq/items").call()?;
    Ok(())
}

pub fn ureq_delete(id: u32) -> Res {
    ureq::delete(&format!("{HOST}/v1/ureq/items/{id}")).call()?;
    Ok(())
}

pub fn ureq_dots() -> Res {
    ureq::get("http://api.example.com/v1/x/../ureq-dots").call()?;
    Ok(())
}

pub fn ureq_agent_head() -> Res {
    let agent: ureq::Agent = ureq::Agent::new_with_defaults();
    agent.head(format!("{HOST}/v1/ureq/health")).call()?;
    Ok(())
}

pub fn ureq_post() -> Res {
    ureq::post("http://api.example.com/v1/ureq/orders").send_empty()?;
    Ok(())
}
