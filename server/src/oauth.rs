//! OAuth login, link, unlink, and backchannel logout. Disabled until `oauth.enabled` is saved.

use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::signature::Verifier;
use rsa::{BigUint, RsaPublicKey};
use serde_json::Value;
use sha2::Sha256;

use crate::db::{
    avatar_color_for_email, new_id, new_update_id, now_iso, random_token, sha256, App,
    ClientDevice, DbError, LoginSuccess, UserDocument,
};
use crate::users::sanitize_filename;

const LOGIN_REDIRECT: &str = "/auth/login?autoLaunch=0";
const GIBIBYTE: i64 = 1024 * 1024 * 1024;
const LOGOUT_EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";
const DEFAULT_LABEL_CLAIM: &str = "preferred_username";

type HmacSha256 = Hmac<Sha256>;

pub(crate) enum OAuthFail {
    BadRequest(String),
    Discovery(String),
    LoginFailed,
    Db(DbError),
}

impl From<DbError> for OAuthFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for OAuthFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Database(error))
    }
}

fn bad(text: impl Into<String>) -> OAuthFail {
    OAuthFail::BadRequest(text.into())
}

struct OAuthSettings {
    enabled: bool,
    auto_register: bool,
    client_id: String,
    client_secret: String,
    issuer_url: String,
    end_session_endpoint: String,
    mobile_override: bool,
    mobile_redirect: String,
    scope: String,
    prompt: String,
    signing_algorithm: String,
    profile_signing_algorithm: String,
    storage_label_claim: String,
    storage_quota_claim: String,
    role_claim: String,
    default_storage_quota: Option<i64>,
    token_auth: String,
    timeout_ms: u64,
    allow_insecure: bool,
}

struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
    end_session_endpoint: Option<String>,
    jwks_uri: Option<String>,
    pkce: bool,
}

pub(crate) struct AuthorizeStart {
    pub url: String,
    pub state: String,
    pub code_verifier: Option<String>,
}

struct OAuthProfile {
    profile: Value,
    sid: Option<String>,
    id_token: Option<String>,
}

impl App {
    pub(crate) async fn oauth_session_bearer(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, OAuthFail> {
        let token: Option<String> =
            sqlx::query_scalar("SELECT oauthBearerToken FROM session WHERE id = ?")
                .bind(session_id)
                .fetch_optional(self.pool())
                .await?
                .flatten();
        Ok(token.filter(|value| !value.is_empty()))
    }

    pub(crate) async fn oauth_logout_redirect(
        &self,
        auth_type: &str,
        bearer: Option<&str>,
    ) -> Result<String, OAuthFail> {
        if auth_type != "oauth" {
            return Ok(LOGIN_REDIRECT.to_string());
        }
        let settings = self.oauth_settings().await?;
        if !settings.enabled {
            return Ok(LOGIN_REDIRECT.to_string());
        }
        let endpoint = if settings.end_session_endpoint.is_empty() {
            self.discover(&settings)
                .await?
                .end_session_endpoint
                .unwrap_or_default()
        } else {
            settings.end_session_endpoint.clone()
        };
        if endpoint.is_empty() {
            return Ok(LOGIN_REDIRECT.to_string());
        }
        Ok(match bearer.filter(|token| !token.is_empty()) {
            Some(token) => add_query(&endpoint, "id_token_hint", token),
            None => endpoint,
        })
    }

    pub(crate) async fn oauth_authorize(
        &self,
        redirect_uri: &str,
        state: Option<&str>,
        code_challenge: Option<&str>,
    ) -> Result<AuthorizeStart, OAuthFail> {
        let settings = self.oauth_settings().await?;
        if !settings.enabled {
            return Err(bad("OAuth is not enabled"));
        }
        let redirect_uri = resolve_redirect(&settings, redirect_uri);
        let discovery = self.discover(&settings).await?;
        let state = state.map(str::to_string).unwrap_or_else(random_secret);
        let (verifier, challenge) = match code_challenge.filter(|value| !value.is_empty()) {
            Some(challenge) => (None, Some(challenge.to_string())),
            None => {
                let verifier = random_secret();
                let challenge = pkce_challenge(&verifier);
                (Some(verifier), Some(challenge))
            }
        };
        let mut pairs = vec![
            ("client_id", settings.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri.as_str()),
            ("scope", settings.scope.as_str()),
            ("state", state.as_str()),
        ];
        if !settings.prompt.is_empty() {
            pairs.push(("prompt", settings.prompt.as_str()));
        }
        if discovery.pkce {
            if let Some(challenge) = challenge.as_deref() {
                pairs.push(("code_challenge", challenge));
                pairs.push(("code_challenge_method", "S256"));
            }
        }
        Ok(AuthorizeStart {
            url: add_queries(&discovery.authorization_endpoint, &pairs),
            state,
            code_verifier: verifier,
        })
    }

    pub(crate) async fn oauth_callback(
        &self,
        url: &str,
        state: Option<&str>,
        verifier: Option<&str>,
        device: &ClientDevice,
    ) -> Result<LoginSuccess, OAuthFail> {
        let settings = self.oauth_settings().await?;
        if !settings.enabled {
            return Err(bad("OAuth is not enabled"));
        }
        let state = required_secret(state, "OAuth state is missing")?;
        let verifier = required_secret(verifier, "OAuth code verifier is missing")?;
        let url = resolve_redirect(&settings, url);
        let profile = self
            .exchange_profile(&settings, &url, state, verifier)
            .await?;
        let user = self.oauth_user(&settings, &profile.profile).await?;
        let user = self.maybe_picture(user, &profile.profile).await;
        let token = self
            .open_oauth_session(
                &user.id,
                device,
                profile.sid.as_deref(),
                profile.id_token.as_deref(),
            )
            .await?;
        Ok(LoginSuccess { token, user })
    }

    pub(crate) async fn oauth_link(
        &self,
        user_id: &str,
        session_id: &str,
        url: &str,
        state: Option<&str>,
        verifier: Option<&str>,
    ) -> Result<UserDocument, OAuthFail> {
        let state = required_secret(state, "OAuth state is missing")?;
        let verifier = required_secret(verifier, "OAuth code verifier is missing")?;
        let settings = self.oauth_settings().await?;
        let profile = self
            .exchange_profile(&settings, url, state, verifier)
            .await?;
        let oauth_id = profile
            .profile
            .get("sub")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| bad("OAuth authentication failed"))?
            .to_string();
        let duplicate: Option<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE oauthId = ? AND deletedAt IS NULL")
                .bind(&oauth_id)
                .fetch_optional(self.pool())
                .await?;
        if duplicate.is_some_and(|id| id != user_id) {
            return Err(bad(
                "This OAuth account has already been linked to another user.",
            ));
        }
        if profile.sid.is_some() || profile.id_token.is_some() {
            sqlx::query(
                "UPDATE session SET oauthSid = COALESCE(?, oauthSid), oauthBearerToken = COALESCE(?, oauthBearerToken) WHERE id = ?",
            )
            .bind(profile.sid.as_deref())
            .bind(profile.id_token.as_deref())
            .bind(session_id)
            .execute(self.pool())
            .await?;
        }
        let now = now_iso();
        sqlx::query("UPDATE user SET oauthId = ?, updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(&oauth_id)
            .bind(&now)
            .bind(new_update_id())
            .bind(user_id)
            .execute(self.pool())
            .await?;
        self.read_user(user_id, false)
            .await?
            .ok_or_else(|| bad("User not found"))
    }

    pub(crate) async fn oauth_unlink(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<UserDocument, OAuthFail> {
        sqlx::query("UPDATE session SET oauthSid = NULL, oauthBearerToken = NULL WHERE id = ?")
            .bind(session_id)
            .execute(self.pool())
            .await?;
        let now = now_iso();
        sqlx::query("UPDATE user SET oauthId = NULL, updatedAt = ?, updateId = ? WHERE id = ?")
            .bind(&now)
            .bind(new_update_id())
            .bind(user_id)
            .execute(self.pool())
            .await?;
        self.read_user(user_id, false)
            .await?
            .ok_or_else(|| bad("User not found"))
    }

    pub(crate) fn mobile_oauth_redirect(url: &str) -> String {
        let query = url.split_once('?').map(|(_, query)| query).unwrap_or("");
        format!("app.immich:///oauth-callback?{query}")
    }

    pub(crate) async fn oauth_backchannel(&self, token: &str) -> Result<(), OAuthFail> {
        let settings = self.oauth_settings().await?;
        if !settings.enabled {
            return Err(bad(
                "Received backchannel logout request but OAuth is not enabled",
            ));
        }
        let claims = match self.logout_claims(&settings, token).await {
            Ok(claims) => claims,
            Err(error) => {
                eprintln!("oauth backchannel: {error}");
                return Err(bad("Error backchannel logout: token validation failed"));
            }
        };
        let Some(claims) = claims else {
            return Err(bad("Invalid logout token: no claims found"));
        };
        let sub = text_claim(&claims, "sub");
        let sid = text_claim(&claims, "sid");
        if sub.is_none() && sid.is_none() {
            return Err(bad(
                "Invalid logout token: it must contain either a sub or a sid claim",
            ));
        }
        let ids = oauth_session_ids(self, sid.as_deref(), sub.as_deref()).await?;
        for id in ids {
            sqlx::query("DELETE FROM session WHERE id = ?")
                .bind(id)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    async fn oauth_settings(&self) -> Result<OAuthSettings, OAuthFail> {
        let config = self.admin_config().await?;
        let oauth = &config["oauth"];
        Ok(OAuthSettings {
            enabled: oauth["enabled"].as_bool().unwrap_or(false),
            auto_register: oauth["autoRegister"].as_bool().unwrap_or(true),
            client_id: oauth["clientId"].as_str().unwrap_or("").to_string(),
            client_secret: oauth["clientSecret"].as_str().unwrap_or("").to_string(),
            issuer_url: oauth["issuerUrl"].as_str().unwrap_or("").to_string(),
            end_session_endpoint: oauth["endSessionEndpoint"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            mobile_override: oauth["mobileOverrideEnabled"].as_bool().unwrap_or(false),
            mobile_redirect: oauth["mobileRedirectUri"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            scope: oauth["scope"]
                .as_str()
                .unwrap_or("openid email profile")
                .to_string(),
            prompt: oauth["prompt"].as_str().unwrap_or("").to_string(),
            signing_algorithm: oauth["signingAlgorithm"]
                .as_str()
                .unwrap_or("RS256")
                .to_string(),
            profile_signing_algorithm: oauth["profileSigningAlgorithm"]
                .as_str()
                .unwrap_or("none")
                .to_string(),
            storage_label_claim: oauth["storageLabelClaim"]
                .as_str()
                .unwrap_or(DEFAULT_LABEL_CLAIM)
                .to_string(),
            storage_quota_claim: oauth["storageQuotaClaim"]
                .as_str()
                .unwrap_or("immich_quota")
                .to_string(),
            role_claim: oauth["roleClaim"]
                .as_str()
                .unwrap_or("immich_role")
                .to_string(),
            default_storage_quota: oauth["defaultStorageQuota"].as_i64(),
            token_auth: oauth["tokenEndpointAuthMethod"]
                .as_str()
                .unwrap_or("client_secret_post")
                .to_string(),
            timeout_ms: oauth["timeout"].as_u64().unwrap_or(30_000).max(1),
            allow_insecure: oauth["allowInsecureRequests"].as_bool().unwrap_or(false),
        })
    }

    async fn discover(&self, settings: &OAuthSettings) -> Result<Discovery, OAuthFail> {
        let issuer = settings.issuer_url.trim_end_matches('/');
        if issuer.is_empty() {
            return Err(OAuthFail::Discovery("invalid issuer URL".to_string()));
        }
        let well_known = format!("{issuer}/.well-known/openid-configuration");
        let document = self.oauth_get_json(settings, &well_known).await?;
        let issuer = document["issuer"].as_str().unwrap_or(issuer).to_string();
        let authorization_endpoint = required_url(&document, "authorization_endpoint")?;
        let token_endpoint = required_url(&document, "token_endpoint")?;
        let pkce = match document.get("code_challenge_methods_supported") {
            None | Some(Value::Null) => true,
            Some(Value::Array(methods)) => {
                methods.iter().any(|method| method.as_str() == Some("S256"))
            }
            Some(_) => true,
        };
        Ok(Discovery {
            issuer,
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint: document["userinfo_endpoint"].as_str().map(str::to_string),
            end_session_endpoint: document["end_session_endpoint"]
                .as_str()
                .map(str::to_string),
            jwks_uri: document["jwks_uri"].as_str().map(str::to_string),
            pkce,
        })
    }

    async fn exchange_profile(
        &self,
        settings: &OAuthSettings,
        url: &str,
        expected_state: &str,
        verifier: &str,
    ) -> Result<OAuthProfile, OAuthFail> {
        let discovery = self.discover(settings).await?;
        let returned_state = query_param(url, "state").unwrap_or_default();
        if returned_state != expected_state {
            eprintln!("oauth: state mismatch");
            return Err(OAuthFail::LoginFailed);
        }
        let Some(code) = query_param(url, "code").filter(|value| !value.is_empty()) else {
            eprintln!("oauth: callback is missing the code");
            return Err(OAuthFail::LoginFailed);
        };
        let redirect_uri = url_without_query(url);
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", settings.client_id.clone()),
        ];
        if discovery.pkce {
            form.push(("code_verifier", verifier.to_string()));
        }
        let mut basic = None;
        if settings.client_secret.is_empty() {
            // A public client sends only the client id.
        } else if settings.token_auth == "client_secret_basic" {
            basic = Some(basic_secret(&settings.client_id, &settings.client_secret));
        } else if settings.token_auth != "none" {
            form.push(("client_secret", settings.client_secret.clone()));
        }
        let tokens = match self
            .oauth_post_form(settings, &discovery.token_endpoint, &form, basic.as_deref())
            .await
        {
            Ok(tokens) => tokens,
            Err(error) => {
                eprintln!("oauth: OAuth login failed: {error}");
                return Err(OAuthFail::LoginFailed);
            }
        };
        self.profile_from_tokens(settings, &discovery, &tokens)
            .await
    }

    async fn profile_from_tokens(
        &self,
        settings: &OAuthSettings,
        discovery: &Discovery,
        tokens: &Value,
    ) -> Result<OAuthProfile, OAuthFail> {
        let id_token = tokens
            .get("id_token")
            .and_then(Value::as_str)
            .map(str::to_string);
        let access_token = tokens
            .get("access_token")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut sid = None;
        let mut profile = None;
        if let Some(token) = &id_token {
            let claims = match self.verify_jwt(settings, discovery, token, false).await {
                Ok(claims) => claims,
                Err(error) => {
                    if error.contains("unexpected JWT alg") {
                        eprintln!(
                            "oauth: Algorithm mismatch. Make sure the signing algorithm is set correctly in the OAuth settings. Or, that you have specified a signing key in your OAuth provider."
                        );
                    }
                    eprintln!("oauth: OAuth login failed: {error}");
                    return Err(OAuthFail::LoginFailed);
                }
            };
            if !access_hash_ok(&claims, access_token) {
                eprintln!("oauth: OAuth login failed: access token hash mismatch");
                return Err(OAuthFail::LoginFailed);
            }
            sid = text_claim(&claims, "sid");
            if claims.get("email").is_some() {
                profile = Some(claims);
            }
        }
        if profile.is_none() {
            let Some(endpoint) = discovery.userinfo_endpoint.as_deref() else {
                eprintln!("oauth: OAuth login failed: no profile");
                return Err(OAuthFail::LoginFailed);
            };
            if access_token.is_empty() {
                eprintln!("oauth: OAuth login failed: no access token");
                return Err(OAuthFail::LoginFailed);
            }
            profile = Some(
                match self.userinfo(settings, endpoint, access_token).await {
                    Ok(profile) => profile,
                    Err(error) => {
                        eprintln!("oauth: OAuth login failed: {error}");
                        return Err(OAuthFail::LoginFailed);
                    }
                },
            );
        }
        let profile = profile.unwrap_or(Value::Null);
        if text_claim(&profile, "sub").is_none() {
            eprintln!("oauth: OAuth login failed: no sub");
            return Err(OAuthFail::LoginFailed);
        }
        Ok(OAuthProfile {
            profile,
            sid,
            id_token,
        })
    }

    async fn oauth_user(
        &self,
        settings: &OAuthSettings,
        profile: &Value,
    ) -> Result<UserDocument, OAuthFail> {
        let sub = text_claim(profile, "sub").ok_or_else(|| bad("OAuth authentication failed"))?;
        let email = normalized_email(profile);
        let mut user_id: Option<String> =
            sqlx::query_scalar("SELECT id FROM user WHERE oauthId = ? AND deletedAt IS NULL")
                .bind(&sub)
                .fetch_optional(self.pool())
                .await?;
        if user_id.is_none() {
            if let Some(email) = &email {
                let row: Option<(String, Option<String>)> = sqlx::query_as(
                    "SELECT id, oauthId FROM user WHERE email = ? AND deletedAt IS NULL",
                )
                .bind(email)
                .fetch_optional(self.pool())
                .await?;
                if let Some((id, linked)) = row {
                    if linked
                        .as_deref()
                        .is_some_and(|value| !value.is_empty() && value != sub)
                    {
                        return Err(bad("OAuth authentication failed"));
                    }
                    if linked.as_deref().unwrap_or("").is_empty() {
                        let now = now_iso();
                        sqlx::query(
                            "UPDATE user SET oauthId = ?, updatedAt = ?, updateId = ? WHERE id = ?",
                        )
                        .bind(&sub)
                        .bind(&now)
                        .bind(new_update_id())
                        .bind(&id)
                        .execute(self.pool())
                        .await?;
                    }
                    user_id = Some(id);
                }
            }
        }
        let is_admin = role_is_admin(profile, &settings.role_claim);
        if let Some(id) = user_id {
            if let Some(is_admin) = is_admin {
                sqlx::query("UPDATE user SET isAdmin = ?, updatedAt = ?, updateId = ? WHERE id = ? AND isAdmin != ?")
                    .bind(i64::from(is_admin))
                    .bind(now_iso())
                    .bind(new_update_id())
                    .bind(&id)
                    .bind(i64::from(is_admin))
                    .execute(self.pool())
                    .await?;
            }
            self.sync_claims(settings, &id, profile).await?;
            return self
                .read_user(&id, false)
                .await?
                .ok_or_else(|| bad("OAuth authentication failed"));
        }
        if !settings.auto_register {
            return Err(bad("OAuth authentication failed"));
        }
        let Some(email) = email else {
            return Err(bad("OAuth profile does not have an email address"));
        };
        self.register_oauth_user(settings, profile, &sub, &email, is_admin.unwrap_or(false))
            .await
    }

    async fn register_oauth_user(
        &self,
        settings: &OAuthSettings,
        profile: &Value,
        sub: &str,
        email: &str,
        is_admin: bool,
    ) -> Result<UserDocument, OAuthFail> {
        let name = profile_name(profile, email);
        let label = storage_label(profile, &settings.storage_label_claim);
        let quota = quota_bytes(profile, &settings.storage_quota_claim)
            .unwrap_or_else(|| default_quota(settings.default_storage_quota));
        let id = new_id();
        let now = now_iso();
        let inserted = sqlx::query(
            "INSERT INTO user (
                id, email, password, pinCode, createdAt, profileImagePath, isAdmin,
                shouldChangePassword, avatarColor, deletedAt, oauthId, updatedAt,
                storageLabel, name, quotaSizeInBytes, quotaUsageInBytes, status,
                profileChangedAt, updateId, clusterGroupId
            ) VALUES (
                ?, ?, NULL, NULL, ?, '', ?,
                1, ?, NULL, ?, ?,
                ?, ?, ?, 0, 'active',
                ?, ?, NULL
            )",
        )
        .bind(&id)
        .bind(email)
        .bind(&now)
        .bind(i64::from(is_admin))
        .bind(avatar_color_for_email(email))
        .bind(sub)
        .bind(&now)
        .bind(label.as_deref())
        .bind(&name)
        .bind(quota)
        .bind(&now)
        .bind(new_update_id())
        .execute(self.pool())
        .await;
        if let Err(error) = inserted {
            if let Some(text) = unique_user_message(&error) {
                return Err(bad(text));
            }
            return Err(error.into());
        }
        self.read_user(&id, false)
            .await?
            .ok_or_else(|| bad("OAuth authentication failed"))
    }

    async fn sync_claims(
        &self,
        settings: &OAuthSettings,
        user_id: &str,
        profile: &Value,
    ) -> Result<(), OAuthFail> {
        let current = self
            .read_user(user_id, false)
            .await?
            .ok_or_else(|| bad("OAuth authentication failed"))?;
        let mut label = None;
        if settings.storage_label_claim != DEFAULT_LABEL_CLAIM {
            if let Some(next) = storage_label(profile, &settings.storage_label_claim) {
                if Some(next.as_str()) != current.storage_label.as_deref() {
                    let taken: Option<String> = sqlx::query_scalar(
                        "SELECT id FROM user WHERE storageLabel = ? AND deletedAt IS NULL AND id != ?",
                    )
                    .bind(&next)
                    .bind(user_id)
                    .fetch_optional(self.pool())
                    .await?;
                    if taken.is_some() {
                        eprintln!("oauth: storage label already in use");
                    } else {
                        label = Some(next);
                    }
                }
            }
        }
        let quota = quota_bytes(profile, &settings.storage_quota_claim)
            .filter(|quota| *quota != current.quota_size_in_bytes);
        if label.is_none() && quota.is_none() {
            return Ok(());
        }
        let mut usage = current.quota_usage_in_bytes;
        if let Some(Some(bytes)) = quota {
            if bytes > 0 {
                usage = recount_usage(self, user_id).await?;
            }
        }
        sqlx::query(
            "UPDATE user SET storageLabel = ?, quotaSizeInBytes = ?, quotaUsageInBytes = ?, updatedAt = ?, updateId = ? WHERE id = ?",
        )
        .bind(label.as_deref().or(current.storage_label.as_deref()))
        .bind(match &quota {
            Some(value) => *value,
            None => current.quota_size_in_bytes,
        })
        .bind(usage)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(user_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    async fn maybe_picture(&self, user: UserDocument, profile: &Value) -> UserDocument {
        if !user.profile_image_path.is_empty() {
            return user;
        }
        let Some(url) = profile.get("picture").and_then(Value::as_str) else {
            return user;
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return user;
        }
        if let Err(error) = self.sync_picture(&user.id, url).await {
            eprintln!("oauth profile picture: {error}");
            return user;
        }
        self.read_user(&user.id, false)
            .await
            .ok()
            .flatten()
            .unwrap_or(user)
    }

    async fn sync_picture(&self, user_id: &str, url: &str) -> Result<(), String> {
        let settings = self
            .oauth_settings()
            .await
            .map_err(|error| format!("{error}"))?;
        let bytes = self
            .oauth_get_bytes(&settings, url, 15 * 1024 * 1024)
            .await?;
        let path = std::env::temp_dir().join(format!("immich-oauth-picture-{user_id}"));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|error| error.to_string())?;
        self.set_profile_image(user_id, &path)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                crate::account::AccountFail::BadRequest(text)
                | crate::account::AccountFail::Unauthorized(text) => text.to_string(),
                crate::account::AccountFail::Db(error) => format!("{error:?}"),
            })
    }

    async fn open_oauth_session(
        &self,
        user_id: &str,
        device: &ClientDevice,
        sid: Option<&str>,
        bearer: Option<&str>,
    ) -> Result<String, OAuthFail> {
        let token = random_token();
        let now = now_iso();
        sqlx::query(
            "INSERT INTO session (
                id, token, createdAt, updatedAt, expiresAt, userId, parentId,
                deviceType, deviceOS, appVersion, updateId, isPendingSyncReset,
                pinExpiresAt, oauthSid, oauthBearerToken
            ) VALUES (?, ?, ?, ?, NULL, ?, NULL, ?, ?, ?, ?, 0, NULL, ?, ?)",
        )
        .bind(new_id())
        .bind(sha256(token.as_bytes()))
        .bind(&now)
        .bind(&now)
        .bind(user_id)
        .bind(&device.device_type)
        .bind(&device.device_os)
        .bind(&device.app_version)
        .bind(new_update_id())
        .bind(sid)
        .bind(bearer)
        .execute(self.pool())
        .await?;
        Ok(token)
    }

    async fn logout_claims(
        &self,
        settings: &OAuthSettings,
        token: &str,
    ) -> Result<Option<Value>, String> {
        let discovery = self.discover(settings).await.map_err(|error| match error {
            OAuthFail::Discovery(text) => text,
            other => format!("{other}"),
        })?;
        let claims = self.verify_jwt(settings, &discovery, token, true).await?;
        let events = claims.get("events").and_then(Value::as_object);
        if events.is_none_or(|events| !events.contains_key(LOGOUT_EVENT)) {
            return Err("Missing backchannel-logout event claim".to_string());
        }
        if claims.get("nonce").is_some_and(|nonce| !nonce.is_null()) {
            return Err("Logout token must not contain a nonce".to_string());
        }
        Ok(Some(claims))
    }

    async fn verify_jwt(
        &self,
        settings: &OAuthSettings,
        discovery: &Discovery,
        token: &str,
        logout: bool,
    ) -> Result<Value, String> {
        let mut parts = token.split('.');
        let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("invalid jwt".to_string());
        };
        let header: Value =
            serde_json::from_slice(&b64url_decode(header_b64).ok_or("invalid jwt header")?)
                .map_err(|_| "invalid jwt header")?;
        let algorithm = header.get("alg").and_then(Value::as_str).unwrap_or("");
        if algorithm != settings.signing_algorithm {
            return Err(format!(
                "unexpected JWT alg received, expected {}",
                settings.signing_algorithm
            ));
        }
        let signing_input = format!("{header_b64}.{payload_b64}");
        let signature = b64url_decode(signature_b64).ok_or("invalid jwt signature")?;
        if algorithm.starts_with("HS") {
            verify_hs(&settings.client_secret, &signing_input, &signature)?;
        } else if algorithm == "RS256" {
            let jwks = discovery
                .jwks_uri
                .as_deref()
                .ok_or("Unable to get JWKS URI")?;
            let keys = self
                .oauth_get_json(settings, jwks)
                .await
                .map_err(|error| error.to_string())?;
            verify_rs256(
                &keys,
                header.get("kid").and_then(Value::as_str),
                &signing_input,
                &signature,
            )?;
        } else {
            return Err(format!("unsupported jwt alg {algorithm}"));
        }
        let payload: Value =
            serde_json::from_slice(&b64url_decode(payload_b64).ok_or("invalid jwt payload")?)
                .map_err(|_| "invalid jwt payload")?;
        if payload.get("iss").and_then(Value::as_str) != Some(discovery.issuer.as_str()) {
            return Err("issuer mismatch".to_string());
        }
        if !audience_matches(&payload, &settings.client_id) {
            return Err("audience mismatch".to_string());
        }
        let max_age = if logout { Some(120) } else { None };
        if !token_time_ok(&payload, max_age) {
            return Err("token time is not valid".to_string());
        }
        Ok(payload)
    }

    async fn userinfo(
        &self,
        settings: &OAuthSettings,
        endpoint: &str,
        access_token: &str,
    ) -> Result<Value, String> {
        let client = oauth_client(settings)?;
        let response = client
            .get(endpoint)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        if settings.profile_signing_algorithm == "none" {
            return serde_json::from_slice(&bytes).map_err(|error| error.to_string());
        }
        let text = String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())?;
        let discovery = self
            .discover(settings)
            .await
            .map_err(|error| error.to_string())?;
        let algorithm = settings.profile_signing_algorithm.clone();
        let profile_settings = settings_alg(settings, &algorithm);
        self.verify_jwt(&profile_settings, &discovery, text.trim(), false)
            .await
    }

    async fn oauth_get_json(
        &self,
        settings: &OAuthSettings,
        url: &str,
    ) -> Result<Value, OAuthFail> {
        let bytes = self
            .oauth_get_bytes(settings, url, 1024 * 1024)
            .await
            .map_err(OAuthFail::Discovery)?;
        serde_json::from_slice(&bytes).map_err(|error| OAuthFail::Discovery(error.to_string()))
    }

    async fn oauth_get_bytes(
        &self,
        settings: &OAuthSettings,
        url: &str,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        require_http(settings, url)?;
        let client = oauth_client(settings)?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("GET {url} returned {}", response.status()));
        }
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        if bytes.len() > limit {
            return Err("response is too large".to_string());
        }
        Ok(bytes.to_vec())
    }

    async fn oauth_post_form(
        &self,
        settings: &OAuthSettings,
        url: &str,
        fields: &[(&str, String)],
        basic: Option<&str>,
    ) -> Result<Value, String> {
        require_http(settings, url)?;
        let client = oauth_client(settings)?;
        let body = form_encode(fields);
        let mut request = client
            .post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body);
        if let Some(basic) = basic {
            request = request.header("authorization", format!("Basic {basic}"));
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!("token endpoint returned {status}"));
        }
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }
}

fn settings_alg(settings: &OAuthSettings, algorithm: &str) -> OAuthSettings {
    OAuthSettings {
        enabled: settings.enabled,
        auto_register: settings.auto_register,
        client_id: settings.client_id.clone(),
        client_secret: settings.client_secret.clone(),
        issuer_url: settings.issuer_url.clone(),
        end_session_endpoint: settings.end_session_endpoint.clone(),
        mobile_override: settings.mobile_override,
        mobile_redirect: settings.mobile_redirect.clone(),
        scope: settings.scope.clone(),
        prompt: settings.prompt.clone(),
        signing_algorithm: algorithm.to_string(),
        profile_signing_algorithm: settings.profile_signing_algorithm.clone(),
        storage_label_claim: settings.storage_label_claim.clone(),
        storage_quota_claim: settings.storage_quota_claim.clone(),
        role_claim: settings.role_claim.clone(),
        default_storage_quota: settings.default_storage_quota,
        token_auth: settings.token_auth.clone(),
        timeout_ms: settings.timeout_ms,
        allow_insecure: settings.allow_insecure,
    }
}

impl std::fmt::Display for OAuthFail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(text) | Self::Discovery(text) => formatter.write_str(text),
            Self::LoginFailed => formatter.write_str("OAuth login failed"),
            Self::Db(error) => write!(formatter, "{error:?}"),
        }
    }
}

fn required_secret<'a>(value: Option<&'a str>, missing: &str) -> Result<&'a str, OAuthFail> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Ok(value),
        None => Err(bad(missing)),
    }
}

fn required_url(document: &Value, key: &str) -> Result<String, OAuthFail> {
    document
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| OAuthFail::Discovery(format!("missing {key}")))
}

fn resolve_redirect(settings: &OAuthSettings, url: &str) -> String {
    if !settings.mobile_override || settings.mobile_redirect.is_empty() {
        return url.to_string();
    }
    let Some((start, end)) = mobile_span(url) else {
        return url.to_string();
    };
    format!(
        "{}{}{}",
        &url[..start],
        settings.mobile_redirect,
        &url[end..]
    )
}

fn mobile_span(url: &str) -> Option<(usize, usize)> {
    let marker = "app.immich:";
    let bytes = url.as_bytes();
    let mut search = 0;
    while let Some(relative) = url[search..].find(marker) {
        let start = search + relative;
        let mut index = start + marker.len();
        if index >= bytes.len() || bytes[index] != b'/' {
            search = start + marker.len();
            continue;
        }
        while index < bytes.len() && bytes[index] == b'/' {
            index += 1;
        }
        if url[index..].starts_with("oauth-callback") {
            return Some((start, index + "oauth-callback".len()));
        }
        search = start + marker.len();
    }
    None
}

fn random_secret() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("os random");
    b64url(&bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    b64url(&sha256(verifier.as_bytes()))
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    if let Ok(bytes) = engine.decode(text) {
        return Some(bytes);
    }
    let pad = (4 - text.len() % 4) % 4;
    let mut padded = text.to_string();
    for _ in 0..pad {
        padded.push('=');
    }
    base64::engine::general_purpose::URL_SAFE
        .decode(padded)
        .ok()
}

fn query_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn add_query(url: &str, key: &str, value: &str) -> String {
    let join = if url.contains('?') { '&' } else { '?' };
    format!("{url}{join}{key}={}", query_encode(value))
}

fn add_queries(url: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = url.to_string();
    for (key, value) in pairs {
        out = add_query(&out, key, value);
    }
    out
}

fn form_encode(fields: &[(&str, String)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", query_encode(key), query_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn basic_secret(client_id: &str, client_secret: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{client_secret}"))
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1.split('#').next()?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(key) == name {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let mut bytes = Vec::new();
    let input = value.as_bytes();
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                bytes.push(byte);
                index += 3;
                continue;
            }
        }
        if input[index] == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(input[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn url_without_query(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .to_string()
}

fn text_claim(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn normalized_email(profile: &Value) -> Option<String> {
    let email = profile.get("email")?.as_str()?.trim().to_lowercase();
    if email.is_empty() {
        None
    } else {
        Some(email)
    }
}

fn profile_name(profile: &Value, email: &str) -> String {
    if let Some(name) = text_claim(profile, "name") {
        return name;
    }
    let given = profile
        .get("given_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let family = profile
        .get("family_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let joined = format!("{given} {family}");
    let joined = joined.trim();
    if !joined.is_empty() {
        return joined.to_string();
    }
    text_claim(profile, "preferred_username").unwrap_or_else(|| email.to_string())
}

fn role_is_admin(profile: &Value, key: &str) -> Option<bool> {
    let value = profile.get(key)?;
    let mut roles = Vec::new();
    if let Some(list) = value.as_array() {
        roles.extend(list.iter().filter_map(Value::as_str));
    } else if let Some(text) = value.as_str() {
        roles.push(text);
    } else {
        return None;
    }
    if roles.contains(&"admin") {
        Some(true)
    } else if roles.contains(&"user") {
        Some(false)
    } else {
        None
    }
}

fn storage_label(profile: &Value, key: &str) -> Option<String> {
    let raw = profile.get(key)?.as_str()?;
    let cleaned = sanitize_filename(raw);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn quota_bytes(profile: &Value, key: &str) -> Option<Option<i64>> {
    let raw = profile.get(key)?;
    let number = if let Some(number) = raw.as_i64() {
        number as f64
    } else if let Some(number) = raw.as_f64() {
        number
    } else if let Some(text) = raw.as_str() {
        text.parse::<f64>().ok()?
    } else {
        return None;
    };
    if !number.is_finite() || (number != -1.0 && number < 0.0) {
        return None;
    }
    if number == -1.0 {
        return Some(None);
    }
    Some((number as i64).checked_mul(GIBIBYTE))
}

fn default_quota(quota: Option<i64>) -> Option<i64> {
    match quota {
        None | Some(-1) => None,
        Some(gib) if gib >= 0 => gib.checked_mul(GIBIBYTE),
        Some(_) => None,
    }
}

fn unique_user_message(error: &sqlx::Error) -> Option<&'static str> {
    let sqlx::Error::Database(database) = error else {
        return None;
    };
    let message = database.message();
    if !(message.contains("2067") || message.contains("1555") || message.contains("UNIQUE")) {
        return None;
    }
    if message.contains("storageLabel") {
        Some("Storage label already in use by another account")
    } else {
        Some("Email is not available")
    }
}

async fn recount_usage(app: &App, user_id: &str) -> Result<i64, OAuthFail> {
    let usage: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(asset_exif.fileSizeInByte), 0)
         FROM asset
         LEFT JOIN asset_exif ON asset_exif.assetId = asset.id
         WHERE asset.libraryId IS NULL AND asset.ownerId = ?",
    )
    .bind(user_id)
    .fetch_one(app.pool())
    .await?;
    Ok(usage)
}

async fn oauth_session_ids(
    app: &App,
    sid: Option<&str>,
    sub: Option<&str>,
) -> Result<Vec<String>, OAuthFail> {
    let ids = match (sid, sub) {
        (Some(sid), Some(sub)) => {
            sqlx::query_scalar(
                "SELECT session.id FROM session
                 JOIN user ON user.id = session.userId
                 WHERE session.oauthSid = ? AND user.oauthId = ?",
            )
            .bind(sid)
            .bind(sub)
            .fetch_all(app.pool())
            .await?
        }
        (None, Some(sub)) => {
            sqlx::query_scalar(
                "SELECT session.id FROM session
                 JOIN user ON user.id = session.userId
                 WHERE user.oauthId = ?",
            )
            .bind(sub)
            .fetch_all(app.pool())
            .await?
        }
        (Some(sid), None) => {
            sqlx::query_scalar("SELECT id FROM session WHERE oauthSid = ?")
                .bind(sid)
                .fetch_all(app.pool())
                .await?
        }
        (None, None) => Vec::new(),
    };
    Ok(ids)
}

fn audience_matches(payload: &Value, client_id: &str) -> bool {
    match payload.get("aud") {
        Some(Value::String(audience)) => audience == client_id,
        Some(Value::Array(audiences)) => audiences
            .iter()
            .any(|audience| audience.as_str() == Some(client_id)),
        _ => false,
    }
}

fn token_time_ok(payload: &Value, max_age: Option<i64>) -> bool {
    let now = chrono::Utc::now().timestamp();
    let Some(exp) = json_i64(payload.get("exp")) else {
        return false;
    };
    if exp + 5 < now {
        return false;
    }
    if let Some(not_before) = json_i64(payload.get("nbf")) {
        if not_before > now + 5 {
            return false;
        }
    }
    if let Some(max_age) = max_age {
        let Some(issued) = json_i64(payload.get("iat")) else {
            return false;
        };
        if issued > now + 5 || now - issued > max_age + 5 {
            return false;
        }
    }
    true
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
}

fn access_hash_ok(payload: &Value, access_token: &str) -> bool {
    let Some(expected) = payload.get("at_hash").and_then(Value::as_str) else {
        return true;
    };
    if access_token.is_empty() {
        return false;
    }
    let digest = sha256(access_token.as_bytes());
    b64url(&digest[..digest.len() / 2]) == expected
}

fn verify_hs(secret: &str, input: &str, signature: &[u8]) -> Result<(), String> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|error| error.to_string())?;
    mac.update(input.as_bytes());
    mac.verify_slice(signature)
        .map_err(|_| "hmac mismatch".to_string())
}

fn verify_rs256(
    keys: &Value,
    kid: Option<&str>,
    input: &str,
    signature: &[u8],
) -> Result<(), String> {
    let list = keys
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("jwks has no keys")?;
    let key = list
        .iter()
        .find(|key| {
            key.get("kty").and_then(Value::as_str) == Some("RSA")
                && kid.is_none_or(|kid| key.get("kid").and_then(Value::as_str) == Some(kid))
        })
        .ok_or("jwks key not found")?;
    let modulus =
        b64url_decode(key.get("n").and_then(Value::as_str).ok_or("jwks n")?).ok_or("jwks n")?;
    let exponent =
        b64url_decode(key.get("e").and_then(Value::as_str).ok_or("jwks e")?).ok_or("jwks e")?;
    let public = RsaPublicKey::new(
        BigUint::from_bytes_be(&modulus),
        BigUint::from_bytes_be(&exponent),
    )
    .map_err(|error| error.to_string())?;
    let verifying = VerifyingKey::<Sha256>::new(public);
    let signature = Signature::try_from(signature).map_err(|error| error.to_string())?;
    verifying
        .verify(input.as_bytes(), &signature)
        .map_err(|_| "rsa mismatch".to_string())
}

fn require_http(settings: &OAuthSettings, url: &str) -> Result<(), String> {
    let scheme = url.split("://").next().unwrap_or("");
    if scheme == "https" || (scheme == "http" && settings.allow_insecure) {
        Ok(())
    } else if scheme == "http" {
        Err("HTTP requests are not allowed".to_string())
    } else {
        Err("invalid URL".to_string())
    }
}

fn oauth_client(settings: &OAuthSettings) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(settings.timeout_ms))
        .build()
        .map_err(|error| error.to_string())
}
