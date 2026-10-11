//! Web approvals for firewall "Ask" decisions (replaces the desktop popup).

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use lr_mcp::gateway::firewall::{FirewallApprovalAction, FirewallManager};
use lr_types::AppError;
use serde::Deserialize;
use std::sync::Arc;

/// Header set by `tailscale serve` with the caller's tailnet login.
pub const OWNER_HEADER: &str = "Tailscale-User-Login";

#[derive(Clone)]
struct AppState {
    firewall: Arc<FirewallManager>,
    owner_login: Arc<String>,
}

#[derive(Deserialize)]
struct DecisionBody {
    action: String,
}

/// Build the approvals router. Every route answers 403 unless the request
/// carries [`OWNER_HEADER`] equal to `owner_login`.
pub fn router(firewall: Arc<FirewallManager>, owner_login: String) -> Router {
    let state = AppState {
        firewall,
        owner_login: Arc::new(owner_login),
    };
    Router::new()
        .route("/", get(index))
        .route("/api/approvals", get(list))
        .route("/api/approvals/{id}", post(decide))
        .layer(middleware::from_fn_with_state(state.clone(), require_owner))
        .with_state(state)
}

async fn require_owner(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(OWNER_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == state.owner_login.as_str());
    if ok {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

async fn list(State(state): State<AppState>) -> Response {
    Json(state.firewall.list_pending()).into_response()
}

async fn decide(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DecisionBody>,
) -> StatusCode {
    let action = match body.action.as_str() {
        "allow_once" => FirewallApprovalAction::AllowOnce,
        "allow_session" => FirewallApprovalAction::AllowSession,
        "deny" => FirewallApprovalAction::Deny,
        "deny_session" => FirewallApprovalAction::DenySession,
        _ => return StatusCode::BAD_REQUEST,
    };
    match state.firewall.submit_response(&id, action, None) {
        Ok(()) => {
            tracing::info!(request_id = %id, action = %body.action, "approval decision");
            StatusCode::NO_CONTENT
        }
        Err(AppError::InvalidParams(_)) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn index() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], INDEX_HTML)
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Approvals</title>
<style>
body { font-family: system-ui, sans-serif; margin: 0; padding: 12px; background: #f5f5f5; color: #111; }
h1 { font-size: 1.2rem; }
.card { background: #fff; border: 1px solid #ccc; border-radius: 8px; padding: 12px; margin-bottom: 12px; }
.row { margin: 4px 0; word-break: break-word; }
.label { color: #555; }
pre { background: #eee; padding: 8px; border-radius: 4px; white-space: pre-wrap; word-break: break-word; margin: 6px 0; }
button { font-size: 1rem; padding: 10px 14px; margin: 4px 4px 0 0; border: 1px solid #888; border-radius: 6px; background: #fff; }
button.allow { background: #d8f0d8; }
button.deny { background: #f6d8d8; }
#status { color: #555; font-size: 0.9rem; }
</style>
</head>
<body>
<h1>Pending approvals</h1>
<div id="status"></div>
<div id="list"></div>
<script>
(function () {
  var listEl = document.getElementById("list");
  var statusEl = document.getElementById("status");

  function row(label, value) {
    var d = document.createElement("div");
    d.className = "row";
    var l = document.createElement("span");
    l.className = "label";
    l.textContent = label + ": ";
    var v = document.createElement("span");
    v.textContent = String(value);
    d.appendChild(l);
    d.appendChild(v);
    return d;
  }

  function decide(id, action) {
    return fetch("/api/approvals/" + encodeURIComponent(id), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action: action })
    }).then(function (r) {
      if (!r.ok && r.status !== 404) { statusEl.textContent = "Error " + r.status; }
      return refresh();
    }).catch(function (e) { statusEl.textContent = "Error: " + e; });
  }

  function button(id, action, text, cls) {
    var b = document.createElement("button");
    b.className = cls;
    b.textContent = text;
    b.addEventListener("click", function () { decide(id, action); });
    return b;
  }

  function render(items) {
    listEl.textContent = "";
    if (items.length === 0) {
      statusEl.textContent = "Nothing pending.";
      return;
    }
    statusEl.textContent = items.length + " pending";
    items.forEach(function (it) {
      var card = document.createElement("div");
      card.className = "card";
      card.appendChild(row("Client", it.client_name));
      card.appendChild(row("Server", it.server_name));
      card.appendChild(row("Tool", it.tool_name));
      var left = Math.max(0, (it.timeout_seconds || 0) - (it.created_at_secs_ago || 0));
      card.appendChild(row("Seconds left", left));
      var pre = document.createElement("pre");
      pre.textContent = it.arguments_preview || "";
      card.appendChild(pre);
      card.appendChild(button(it.request_id, "allow_once", "Allow once", "allow"));
      card.appendChild(button(it.request_id, "allow_session", "Allow session", "allow"));
      card.appendChild(button(it.request_id, "deny", "Deny", "deny"));
      card.appendChild(button(it.request_id, "deny_session", "Deny session", "deny"));
      listEl.appendChild(card);
    });
  }

  function refresh() {
    return fetch("/api/approvals").then(function (r) {
      if (!r.ok) { throw new Error("HTTP " + r.status); }
      return r.json();
    }).then(render).catch(function (e) { statusEl.textContent = "Error: " + e; });
  }

  refresh();
  setInterval(refresh, 3000);
})();
</script>
</body>
</html>
"##;
