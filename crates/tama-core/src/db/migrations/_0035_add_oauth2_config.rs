/// v35 — Add OAuth2 configuration columns to app_proxy
pub const MIGRATION: (i32, bool, &str) = (
    35,
    false,
    r#"
        ALTER TABLE app_proxy ADD COLUMN oauth2_enabled INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE app_proxy ADD COLUMN oauth2_client_id TEXT NOT NULL DEFAULT '';
        ALTER TABLE app_proxy ADD COLUMN oauth2_client_secret TEXT NOT NULL DEFAULT '';
        ALTER TABLE app_proxy ADD COLUMN oauth2_authorize_url TEXT NOT NULL DEFAULT '';
        ALTER TABLE app_proxy ADD COLUMN oauth2_token_url TEXT NOT NULL DEFAULT '';
        ALTER TABLE app_proxy ADD COLUMN oauth2_userinfo_url TEXT;
        ALTER TABLE app_proxy ADD COLUMN oauth2_logout_url TEXT;
        ALTER TABLE app_proxy ADD COLUMN oauth2_redirect_uri TEXT NOT NULL DEFAULT '';
        ALTER TABLE app_proxy ADD COLUMN oauth2_scopes TEXT NOT NULL DEFAULT '["openid","profile","email"]';
        ALTER TABLE app_proxy ADD COLUMN oauth2_session_ttl_secs INTEGER NOT NULL DEFAULT 86400;
    "#,
);
