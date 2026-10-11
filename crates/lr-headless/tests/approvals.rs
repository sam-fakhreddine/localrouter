//! RED tests for the web approvals API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use lr_headless::approvals::{router, OWNER_HEADER};
use lr_mcp::gateway::firewall::{FirewallApprovalAction, FirewallApprovalResponse, FirewallManager};
use lr_types::AppResult;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tower::ServiceExt;

const OWNER: &str = "owner@example.com";

fn setup() -> (Arc<FirewallManager>, Router) {
    let fw = Arc::new(FirewallManager::new(300));
    let app = router(fw.clone(), OWNER.to_string());
    (fw, app)
}

fn spawn_request(
    fw: &Arc<FirewallManager>,
    timeout: Option<u64>,
) -> JoinHandle<AppResult<FirewallApprovalResponse>> {
    let fw = fw.clone();
    tokio::spawn(async move {
        fw.request_approval(
            "client-1".to_string(),
            "Test Client".to_string(),
            "write_file".to_string(),
            "files-server".to_string(),
            "path=/etc/hosts".to_string(),
            timeout,
            None,
        )
        .await
    })
}

async fn wait_pending(fw: &Arc<FirewallManager>) -> String {
    for _ in 0..200 {
        let pending = fw.list_pending();
        if pending.len() == 1 {
            return pending[0].request_id.clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("request never became pending");
}

fn req(method: &str, uri: &str, login: Option<&str>, body: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(l) = login {
        b = b.header(OWNER_HEADER, l);
    }
    if body.is_some() {
        b = b.header(header::CONTENT_TYPE, "application/json");
    }
    b.body(Body::from(body.unwrap_or("").to_string())).unwrap()
}

async fn post_action(app: &Router, id: &str, login: Option<&str>, action: &str) -> StatusCode {
    let body = format!("{{\"action\":\"{action}\"}}");
    let uri = format!("/api/approvals/{id}");
    app.clone().oneshot(req("POST", &uri, login, Some(&body))).await.unwrap().status()
}

mod test_properties {
    use super::*;

    #[tokio::test]
    async fn lists_pending_request() {
        // Catches: Availability LIVENESS: owner cannot see the pending request details
        let (fw, app) = setup();
        let _task = spawn_request(&fw, Some(300));
        let id = wait_pending(&fw).await;
        let resp = app.oneshot(req("GET", "/api/approvals", Some(OWNER), None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["request_id"], serde_json::Value::String(id));
        assert_eq!(arr[0]["client_id"], "client-1");
        assert_eq!(arr[0]["tool_name"], "write_file");
        assert_eq!(arr[0]["server_name"], "files-server");
    }

    #[tokio::test]
    async fn allow_once_resolves_request() {
        // Catches: Integrity SAFETY: allow_once answer is dropped or mapped to another action
        let (fw, app) = setup();
        let task = spawn_request(&fw, Some(300));
        let id = wait_pending(&fw).await;
        assert_eq!(post_action(&app, &id, Some(OWNER), "allow_once").await, StatusCode::NO_CONTENT);
        let resp = task.await.unwrap().unwrap();
        assert_eq!(resp.action, FirewallApprovalAction::AllowOnce);
    }

    #[tokio::test]
    async fn deny_resolves_request() {
        // Catches: Integrity SAFETY: deny answer is mapped to an allow action
        let (fw, app) = setup();
        let task = spawn_request(&fw, Some(300));
        let id = wait_pending(&fw).await;
        assert_eq!(post_action(&app, &id, Some(OWNER), "deny").await, StatusCode::NO_CONTENT);
        let resp = task.await.unwrap().unwrap();
        assert_eq!(resp.action, FirewallApprovalAction::Deny);
    }

    #[tokio::test]
    async fn rejects_action_outside_the_four() {
        // Catches: Integrity SAFETY: web API grants a permanent action
        let (fw, app) = setup();
        let _task = spawn_request(&fw, Some(300));
        let id = wait_pending(&fw).await;
        assert_eq!(
            post_action(&app, &id, Some(OWNER), "allow_permanent").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(fw.list_pending().len(), 1);
    }

    #[tokio::test]
    async fn unknown_id_is_404() {
        // Catches: Integrity SAFETY: unknown request id reports success
        let (_fw, app) = setup();
        assert_eq!(
            post_action(&app, "no-such-id", Some(OWNER), "deny").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn non_owner_gets_403_on_every_route() {
        // Catches: Isolation SAFETY: a non-owner can read or answer approvals
        let (fw, app) = setup();
        let _task = spawn_request(&fw, Some(300));
        let id = wait_pending(&fw).await;
        let post_uri = format!("/api/approvals/{id}");
        for login in [None, Some("someone@else")] {
            for (method, uri, body) in [
                ("GET", "/", None),
                ("GET", "/api/approvals", None),
                ("POST", post_uri.as_str(), Some("{\"action\":\"allow_once\"}")),
            ] {
                let resp = app.clone().oneshot(req(method, uri, login, body)).await.unwrap();
                assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{method} {uri} {login:?}");
            }
        }
        assert_eq!(fw.list_pending().len(), 1);
    }

    #[tokio::test]
    async fn index_page_is_html() {
        // Catches: Availability LIVENESS: owner gets no HTML approvals page
        let (_fw, app) = setup();
        let resp = app.oneshot(req("GET", "/", Some(OWNER), None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(ct.starts_with("text/html"), "content type was {ct}");
    }

    #[tokio::test]
    async fn unanswered_request_times_out_to_deny() {
        // Catches: Availability LIVENESS: an unanswered request hangs or auto-allows
        let (fw, app) = setup();
        let task = spawn_request(&fw, Some(1));
        let _ = wait_pending(&fw).await;
        let resp = task.await.unwrap().unwrap();
        assert_eq!(resp.action, FirewallApprovalAction::Deny);
        let resp = app.oneshot(req("GET", "/api/approvals", Some(OWNER), None)).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }
}
