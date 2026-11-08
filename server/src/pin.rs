//! PIN code and the elevated session the locked folder reads.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::db::{new_update_id, now_iso, App, DbError};

pub(crate) enum PinFail {
    Db(DbError),
    BadRequest(&'static str),
    Unauthorized(&'static str),
}

impl From<DbError> for PinFail {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<sqlx::Error> for PinFail {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

struct PinRow {
    pin_code: Option<String>,
    password: Option<String>,
}

impl App {
    pub(crate) async fn auth_status(
        &self,
        user_id: &str,
        session: Option<&crate::db::CurrentSession>,
    ) -> Result<Value, PinFail> {
        let Some(row) = self.pin_row(user_id).await? else {
            return Err(PinFail::Unauthorized("Unauthorized"));
        };
        let mut body = json!({
            "pinCode": present(row.pin_code.as_deref()),
            "password": present(row.password.as_deref()),
            "isElevated": session.is_some_and(|session| session.elevated),
        });
        if let Some(session) = session {
            if let Some(expires_at) = &session.expires_at {
                body["expiresAt"] = json!(expires_at);
            }
            if let Some(pin_expires_at) = &session.pin_expires_at {
                body["pinExpiresAt"] = json!(pin_expires_at);
            }
        }
        Ok(body)
    }

    pub(crate) async fn setup_pin(&self, user_id: &str, pin: &str) -> Result<(), PinFail> {
        let Some(row) = self.pin_row(user_id).await? else {
            return Err(PinFail::Unauthorized("Unauthorized"));
        };
        if present(row.pin_code.as_deref()) {
            return Err(PinFail::BadRequest("User already has a PIN code"));
        }
        self.store_pin(user_id, Some(&hash_pin(pin)?)).await
    }

    pub(crate) async fn change_pin(
        &self,
        user_id: &str,
        password: Option<&str>,
        pin: Option<&str>,
        new_pin: &str,
    ) -> Result<(), PinFail> {
        self.check_current(user_id, password, pin).await?;
        self.store_pin(user_id, Some(&hash_pin(new_pin)?)).await
    }

    pub(crate) async fn reset_pin(
        &self,
        user_id: &str,
        password: Option<&str>,
        pin: Option<&str>,
    ) -> Result<(), PinFail> {
        self.check_current(user_id, password, pin).await?;
        self.store_pin(user_id, None).await?;
        sqlx::query("UPDATE session SET pinExpiresAt = NULL WHERE userId = ?")
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn unlock_session(
        &self,
        user_id: &str,
        session_id: &str,
        pin: Option<&str>,
    ) -> Result<(), PinFail> {
        self.check_current(user_id, None, pin).await?;
        let expires = (Utc::now() + chrono::Duration::minutes(15))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        sqlx::query("UPDATE session SET pinExpiresAt = ? WHERE id = ?")
            .bind(&expires)
            .bind(session_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn lock_session(&self, session_id: &str) -> Result<(), PinFail> {
        sqlx::query("UPDATE session SET pinExpiresAt = NULL WHERE id = ?")
            .bind(session_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn check_current(
        &self,
        user_id: &str,
        password: Option<&str>,
        pin: Option<&str>,
    ) -> Result<(), PinFail> {
        let Some(row) = self.pin_row(user_id).await? else {
            return Err(PinFail::Unauthorized("Unauthorized"));
        };
        let Some(stored_pin) = row.pin_code.as_deref().filter(|value| !value.is_empty()) else {
            return Err(PinFail::BadRequest("User does not have a PIN code"));
        };
        if let Some(password) = password.filter(|value| !value.is_empty()) {
            let matches = row
                .password
                .as_deref()
                .is_some_and(|hash| bcrypt::verify(password, hash).unwrap_or(false));
            if !matches {
                return Err(PinFail::BadRequest("Wrong password"));
            }
            return Ok(());
        }
        if let Some(pin) = pin.filter(|value| !value.is_empty()) {
            if !bcrypt::verify(pin, stored_pin).unwrap_or(false) {
                return Err(PinFail::BadRequest("Wrong PIN code"));
            }
            return Ok(());
        }
        Err(PinFail::BadRequest(
            "Either password or pinCode is required",
        ))
    }

    async fn store_pin(&self, user_id: &str, pin: Option<&str>) -> Result<(), PinFail> {
        let changed = sqlx::query(
            "UPDATE user SET pinCode = ?, updatedAt = ?, updateId = ? WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(pin)
        .bind(now_iso())
        .bind(new_update_id())
        .bind(user_id)
        .execute(self.pool())
        .await?;
        if changed.rows_affected() == 0 {
            return Err(PinFail::Unauthorized("Unauthorized"));
        }
        Ok(())
    }

    async fn pin_row(&self, user_id: &str) -> Result<Option<PinRow>, DbError> {
        let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT pinCode, password FROM user WHERE id = ? AND deletedAt IS NULL",
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|(pin_code, password)| PinRow { pin_code, password }))
    }
}

fn present(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

fn hash_pin(pin: &str) -> Result<String, PinFail> {
    bcrypt::hash(pin, 10).map_err(|error| DbError::Store(error.to_string()).into())
}
