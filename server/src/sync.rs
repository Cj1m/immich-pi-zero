//! Timeline sync. Each line is `{type, data, ack}` and the body ends with `SyncCompleteV1`.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::db::{avatar_color_for_email, new_update_id, App, DbError};
use crate::media::{sync_asset_json, sync_exif_json};

const REQUEST_ORDER: &[&str] = &[
    "AuthUsersV1",
    "AuthUsersV2",
    "UsersV1",
    "PartnersV1",
    "AssetsV1",
    "AssetsV2",
    "StacksV1",
    "PartnerAssetsV1",
    "PartnerAssetsV2",
    "PartnerStacksV1",
    "AlbumAssetsV1",
    "AlbumAssetsV2",
    "AlbumsV1",
    "AlbumsV2",
    "AlbumUsersV1",
    "AlbumToAssetsV1",
    "AssetExifsV1",
    "AlbumAssetExifsV1",
    "AssetOcrV1",
    "PartnerAssetExifsV1",
    "MemoriesV1",
    "MemoriesV2",
    "MemoryToAssetsV1",
    "MemoryToAssetsV2",
    "PeopleV1",
    "AssetFacesV1",
    "AssetFacesV2",
    "AssetFacesV3",
    "UserMetadataV1",
    "AssetMetadataV1",
    "AssetEditsV1",
];

const FULL_SYNC_MS: i64 = 30 * 24 * 60 * 60 * 1000;

impl App {
    pub async fn sync_stream(
        &self,
        session_id: &str,
        user_id: &str,
        types: &[String],
        reset: bool,
    ) -> Result<String, DbError> {
        if reset {
            self.clear_checkpoints(session_id).await?;
        }
        let checkpoints = self.checkpoint_map(session_id).await?;
        if sync_is_stale(&checkpoints) {
            return Ok(sync_line("SyncResetV1", json!({}), "reset"));
        }
        let now_id = new_update_id();
        let mut body = String::new();
        for request in REQUEST_ORDER {
            if !types.iter().any(|item| item == request) {
                continue;
            }
            match *request {
                "AuthUsersV1" => {
                    self.write_auth_user(
                        &mut body,
                        user_id,
                        "AuthUserV1",
                        true,
                        &checkpoints,
                        &now_id,
                    )
                    .await?;
                }
                "AuthUsersV2" => {
                    self.write_auth_user(
                        &mut body,
                        user_id,
                        "AuthUserV2",
                        false,
                        &checkpoints,
                        &now_id,
                    )
                    .await?;
                }
                "UsersV1" => {
                    self.write_user(&mut body, user_id, &checkpoints, &now_id)
                        .await?;
                }
                "AssetsV2" => {
                    self.write_assets(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "AssetExifsV1" => {
                    self.write_exifs(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "AlbumsV2" => {
                    self.write_albums(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "AlbumUsersV1" => {
                    self.write_album_users(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "AlbumToAssetsV1" => {
                    self.write_album_links(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "StacksV1" => {
                    self.write_stacks(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "MemoriesV2" => {
                    self.write_memories(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "MemoryToAssetsV2" => {
                    self.write_memory_assets(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "PeopleV1" => {
                    self.write_people(&mut body, user_id, &checkpoints, &now_id)
                        .await?
                }
                "AssetFacesV2" => {
                    self.write_faces(&mut body, user_id, &checkpoints, &now_id, false)
                        .await?
                }
                "AssetFacesV3" => {
                    self.write_faces(&mut body, user_id, &checkpoints, &now_id, true)
                        .await?
                }
                _ => {}
            }
        }
        body.push_str(&sync_line("SyncCompleteV1", json!({}), &now_id));
        Ok(body)
    }

    pub async fn sync_ack_list(&self, session_id: &str) -> Result<Value, DbError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT type, ack FROM sync_checkpoint WHERE sessionId = ?")
                .bind(session_id)
                .fetch_all(self.pool())
                .await?;
        Ok(rows
            .into_iter()
            .map(|(kind, ack)| json!({ "type": kind, "ack": ack }))
            .collect())
    }

    pub async fn set_sync_acks(&self, session_id: &str, acks: &[String]) -> Result<(), DbError> {
        if acks.len() > 1000 {
            return Err(DbError::Validation);
        }
        for ack in acks {
            let Some((kind, update_id)) = ack.split_once('|') else {
                return Err(DbError::Validation);
            };
            if kind == "SyncResetV1" {
                self.clear_checkpoints(session_id).await?;
                return Ok(());
            }
            if !is_entity_type(kind) || update_id.is_empty() {
                return Err(DbError::Validation);
            }
            sqlx::query(
                "INSERT INTO sync_checkpoint (sessionId, type, ack) VALUES (?, ?, ?)
                 ON CONFLICT(sessionId, type) DO UPDATE SET ack = excluded.ack",
            )
            .bind(session_id)
            .bind(kind)
            .bind(ack)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    pub async fn delete_sync_acks(
        &self,
        session_id: &str,
        types: Option<&[String]>,
    ) -> Result<(), DbError> {
        let Some(types) = types else {
            self.clear_checkpoints(session_id).await?;
            return Ok(());
        };
        for kind in types {
            sqlx::query("DELETE FROM sync_checkpoint WHERE sessionId = ? AND type = ?")
                .bind(session_id)
                .bind(kind)
                .execute(self.pool())
                .await?;
        }
        Ok(())
    }

    async fn clear_checkpoints(&self, session_id: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM sync_checkpoint WHERE sessionId = ?")
            .bind(session_id)
            .execute(self.pool())
            .await?;
        sqlx::query("UPDATE session SET isPendingSyncReset = 0 WHERE id = ?")
            .bind(session_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn checkpoint_map(&self, session_id: &str) -> Result<HashMap<String, String>, DbError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT type, ack FROM sync_checkpoint WHERE sessionId = ?")
                .bind(session_id)
                .fetch_all(self.pool())
                .await?;
        Ok(rows.into_iter().collect())
    }

    async fn write_auth_user(
        &self,
        body: &mut String,
        user_id: &str,
        kind: &str,
        empty_oauth: bool,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let Some(user) = self
            .user_sync_row(user_id, cursor(checkpoints, kind), now_id)
            .await?
        else {
            return Ok(());
        };
        let mut data = user_sync_json(&user, true);
        if empty_oauth && data["oauthId"].is_null() {
            data["oauthId"] = json!("");
        }
        body.push_str(&sync_line(kind, data, &user.update_id));
        Ok(())
    }

    async fn write_user(
        &self,
        body: &mut String,
        _user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes: Vec<(String, String)> = sqlx::query_as(
            "SELECT updateId, userId FROM user_audit
             WHERE (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(cursor(checkpoints, "UserDeleteV1"))
        .bind(cursor(checkpoints, "UserDeleteV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, deleted_id) in deletes {
            body.push_str(&sync_line(
                "UserDeleteV1",
                json!({ "userId": deleted_id }),
                &update_id,
            ));
        }
        // Invite-people reads every user, not only the caller.
        let users = self
            .changed_users(cursor(checkpoints, "UserV1"), now_id)
            .await?;
        for user in users {
            body.push_str(&sync_line(
                "UserV1",
                user_sync_json(&user, false),
                &user.update_id,
            ));
        }
        Ok(())
    }

    async fn write_assets(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes = self
            .audit_ids(
                "SELECT updateId, assetId FROM asset_audit WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ? ORDER BY updateId",
                user_id,
                cursor(checkpoints, "AssetDeleteV1"),
                now_id,
            )
            .await?;
        for (update_id, asset_id) in deletes {
            body.push_str(&sync_line(
                "AssetDeleteV1",
                json!({ "assetId": asset_id }),
                &update_id,
            ));
        }
        let assets = self
            .changed_assets(user_id, cursor(checkpoints, "AssetV2"), now_id)
            .await?;
        for asset in assets {
            let update_id = asset.update_id.clone();
            body.push_str(&sync_line("AssetV2", sync_asset_json(&asset), &update_id));
        }
        Ok(())
    }

    async fn write_exifs(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let assets = self
            .changed_assets(user_id, cursor(checkpoints, "AssetExifV1"), now_id)
            .await?;
        for asset in assets {
            // Exif shares the asset update id so a metadata change and the asset move together.
            let update_id = asset.update_id.clone();
            body.push_str(&sync_line(
                "AssetExifV1",
                sync_exif_json(&asset),
                &update_id,
            ));
        }
        Ok(())
    }

    async fn write_albums(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes = self
            .audit_ids(
                "SELECT updateId, albumId FROM album_audit WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ? ORDER BY updateId",
                user_id,
                cursor(checkpoints, "AlbumDeleteV1"),
                now_id,
            )
            .await?;
        for (update_id, album_id) in deletes {
            body.push_str(&sync_line(
                "AlbumDeleteV1",
                json!({ "albumId": album_id }),
                &update_id,
            ));
        }
        let rows = sqlx::query_as::<_, SyncAlbum>(
            "SELECT updateId AS update_id, id, albumName AS name, description,
                    createdAt AS created_at, updatedAt AS updated_at,
                    albumThumbnailAssetId AS thumbnail_asset_id,
                    isActivityEnabled AS is_activity_enabled, albumOrder AS album_order
             FROM album
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "AlbumV2"))
        .bind(cursor(checkpoints, "AlbumV2"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for album in rows {
            body.push_str(&sync_line(
                "AlbumV2",
                json!({
                    "id": album.id,
                    "name": album.name,
                    "description": album.description,
                    "createdAt": album.created_at,
                    "updatedAt": album.updated_at,
                    "thumbnailAssetId": album.thumbnail_asset_id,
                    "isActivityEnabled": album.is_activity_enabled != 0,
                    "order": album.album_order,
                }),
                &album.update_id,
            ));
        }
        Ok(())
    }

    async fn write_album_users(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT updateId, albumId, userId FROM album_user_audit
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "AlbumUserDeleteV1"))
        .bind(cursor(checkpoints, "AlbumUserDeleteV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, album_id, member_id) in deletes {
            body.push_str(&sync_line(
                "AlbumUserDeleteV1",
                json!({ "albumId": album_id, "userId": member_id }),
                &update_id,
            ));
        }
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT album_user.updateId, album_user.albumId, album_user.userId, album_user.role
             FROM album_user JOIN album ON album.id = album_user.albumId
             WHERE album.ownerId = ? AND (? IS NULL OR album_user.updateId > ?) AND album_user.updateId < ?
             ORDER BY album_user.updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "AlbumUserV1"))
        .bind(cursor(checkpoints, "AlbumUserV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, album_id, member_id, role) in rows {
            body.push_str(&sync_line(
                "AlbumUserV1",
                json!({ "albumId": album_id, "userId": member_id, "role": role }),
                &update_id,
            ));
        }
        Ok(())
    }

    async fn write_album_links(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT updateId, albumId, assetId FROM album_asset_audit
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "AlbumToAssetDeleteV1"))
        .bind(cursor(checkpoints, "AlbumToAssetDeleteV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, album_id, asset_id) in deletes {
            body.push_str(&sync_line(
                "AlbumToAssetDeleteV1",
                json!({ "albumId": album_id, "assetId": asset_id }),
                &update_id,
            ));
        }
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT album_asset.updateId, album_asset.albumId, album_asset.assetId
             FROM album_asset JOIN album ON album.id = album_asset.albumId
             WHERE album.ownerId = ? AND (? IS NULL OR album_asset.updateId > ?) AND album_asset.updateId < ?
             ORDER BY album_asset.updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "AlbumToAssetV1"))
        .bind(cursor(checkpoints, "AlbumToAssetV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, album_id, asset_id) in rows {
            body.push_str(&sync_line(
                "AlbumToAssetV1",
                json!({ "albumId": album_id, "assetId": asset_id }),
                &update_id,
            ));
        }
        Ok(())
    }

    async fn write_stacks(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT updateId, id, createdAt, updatedAt, primaryAssetId
             FROM stack
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "StackV1"))
        .bind(cursor(checkpoints, "StackV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, id, created_at, updated_at, primary) in rows {
            body.push_str(&sync_line(
                "StackV1",
                json!({
                    "id": id,
                    "createdAt": created_at,
                    "updatedAt": updated_at,
                    "primaryAssetId": primary,
                    "ownerId": user_id,
                }),
                &update_id,
            ));
        }
        Ok(())
    }

    async fn write_memories(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes = self
            .audit_ids(
                "SELECT updateId, memoryId FROM memory_audit
                 WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
                 ORDER BY updateId",
                user_id,
                cursor(checkpoints, "MemoryDeleteV1"),
                now_id,
            )
            .await?;
        for (update_id, memory_id) in deletes {
            body.push_str(&sync_line(
                "MemoryDeleteV1",
                json!({ "memoryId": memory_id }),
                &update_id,
            ));
        }
        let rows = sqlx::query_as::<_, SyncMemory>(
            "SELECT updateId AS update_id, id, createdAt AS created_at, updatedAt AS updated_at,
                    deletedAt AS deleted_at, ownerId AS owner_id, type AS kind, data,
                    isSaved AS is_saved, memoryAt AS memory_at, seenAt AS seen_at,
                    showAt AS show_at, hideAt AS hide_at
             FROM memory
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "MemoryV2"))
        .bind(cursor(checkpoints, "MemoryV2"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for memory in rows {
            let data = serde_json::from_str::<Value>(&memory.data).unwrap_or_else(|_| json!({}));
            body.push_str(&sync_line(
                "MemoryV2",
                json!({
                    "id": memory.id,
                    "createdAt": memory.created_at,
                    "updatedAt": memory.updated_at,
                    "deletedAt": memory.deleted_at,
                    "ownerId": memory.owner_id,
                    "type": memory.kind,
                    "data": data,
                    "isSaved": memory.is_saved != 0,
                    "memoryAt": memory.memory_at,
                    "seenAt": memory.seen_at,
                    "showAt": memory.show_at,
                    "hideAt": memory.hide_at,
                }),
                &memory.update_id,
            ));
        }
        Ok(())
    }

    async fn write_memory_assets(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT updateId, memoryId, assetId FROM memory_asset_audit
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "MemoryToAssetDeleteV1"))
        .bind(cursor(checkpoints, "MemoryToAssetDeleteV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, memory_id, asset_id) in deletes {
            body.push_str(&sync_line(
                "MemoryToAssetDeleteV1",
                json!({ "memoryId": memory_id, "assetId": asset_id }),
                &update_id,
            ));
        }
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT memory_asset.updateId, memory_asset.memoryId, memory_asset.assetId
             FROM memory_asset
             JOIN memory ON memory.id = memory_asset.memoryId
             WHERE memory.ownerId = ? AND (? IS NULL OR memory_asset.updateId > ?)
               AND memory_asset.updateId < ?
             ORDER BY memory_asset.updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "MemoryToAssetV2"))
        .bind(cursor(checkpoints, "MemoryToAssetV2"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for (update_id, memory_id, asset_id) in rows {
            body.push_str(&sync_line(
                "MemoryToAssetV2",
                json!({ "memoryId": memory_id, "assetId": asset_id }),
                &update_id,
            ));
        }
        Ok(())
    }

    async fn write_people(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
    ) -> Result<(), DbError> {
        let deletes: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, personGroupId FROM person_audit
             WHERE ownerId = ? AND id < ? AND (? IS NULL OR id > ?)
             ORDER BY id",
        )
        .bind(user_id)
        .bind(now_id)
        .bind(cursor(checkpoints, "PersonDeleteV1"))
        .bind(cursor(checkpoints, "PersonDeleteV1"))
        .fetch_all(self.pool())
        .await?;
        for (audit_id, person_id) in deletes {
            body.push_str(&sync_line(
                "PersonDeleteV1",
                json!({ "personId": person_id }),
                &audit_id,
            ));
        }
        let rows: Vec<SyncPerson> = sqlx::query_as(
            "SELECT updateId AS update_id, personGroupId AS id, createdAt AS created_at, updatedAt AS updated_at,
                    ownerId AS owner_id, name, birthDate AS birth_date, isHidden AS is_hidden,
                    isFavorite AS is_favorite, color, faceAssetId AS face_asset_id
             FROM person
             WHERE ownerId = ? AND (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(user_id)
        .bind(cursor(checkpoints, "PersonV1"))
        .bind(cursor(checkpoints, "PersonV1"))
        .bind(now_id)
        .fetch_all(self.pool())
        .await?;
        for person in rows {
            body.push_str(&sync_line(
                "PersonV1",
                json!({
                    "id": person.id,
                    "createdAt": person.created_at,
                    "updatedAt": person.updated_at,
                    "ownerId": person.owner_id,
                    "name": person.name,
                    "birthDate": sync_birth(person.birth_date),
                    "isHidden": person.is_hidden != 0,
                    "isFavorite": person.is_favorite != 0,
                    "color": person.color,
                    "faceAssetId": person.face_asset_id,
                }),
                &person.update_id,
            ));
        }
        Ok(())
    }

    async fn write_faces(
        &self,
        body: &mut String,
        user_id: &str,
        checkpoints: &HashMap<String, String>,
        now_id: &str,
        cluster: bool,
    ) -> Result<(), DbError> {
        let face_type = if cluster {
            "AssetFaceV3"
        } else {
            "AssetFaceV2"
        };
        let delete_sql = format!(
            "SELECT asset_face_audit.id, asset_face_audit.assetFaceId
             FROM asset_face_audit
             JOIN asset ON asset.id = asset_face_audit.assetId
             JOIN user AS owner ON owner.id = asset.ownerId
             WHERE {FACE_OWNER}
               AND asset_face_audit.id < ?
               AND (? IS NULL OR asset_face_audit.id > ?)
             ORDER BY asset_face_audit.id"
        );
        let deletes: Vec<(String, String)> = sqlx::query_as(&delete_sql)
            .bind(user_id)
            .bind(i64::from(cluster))
            .bind(user_id)
            .bind(now_id)
            .bind(cursor(checkpoints, "AssetFaceDeleteV1"))
            .bind(cursor(checkpoints, "AssetFaceDeleteV1"))
            .fetch_all(self.pool())
            .await?;
        for (audit_id, face_id) in deletes {
            body.push_str(&sync_line(
                "AssetFaceDeleteV1",
                json!({ "assetFaceId": face_id }),
                &audit_id,
            ));
        }
        let face_sql = format!(
            "SELECT asset_face.updateId AS update_id, asset_face.id AS id, asset_face.assetId AS asset_id,
                    asset_face.personGroupId AS person_id, asset_face.imageWidth AS image_width,
                    asset_face.imageHeight AS image_height, asset_face.boundingBoxX1 AS x1,
                    asset_face.boundingBoxY1 AS y1, asset_face.boundingBoxX2 AS x2, asset_face.boundingBoxY2 AS y2,
                    asset_face.sourceType AS source_type, asset_face.isVisible AS is_visible,
                    asset_face.deletedAt AS deleted_at
             FROM asset_face
             JOIN asset ON asset.id = asset_face.assetId
             JOIN user AS owner ON owner.id = asset.ownerId
             WHERE {FACE_OWNER}
               AND (? IS NULL OR asset_face.updateId > ?)
               AND asset_face.updateId < ?
             ORDER BY asset_face.updateId"
        );
        let rows: Vec<SyncFace> = sqlx::query_as(&face_sql)
            .bind(user_id)
            .bind(i64::from(cluster))
            .bind(user_id)
            .bind(cursor(checkpoints, face_type))
            .bind(cursor(checkpoints, face_type))
            .bind(now_id)
            .fetch_all(self.pool())
            .await?;
        for face in rows {
            body.push_str(&sync_line(
                face_type,
                json!({
                    "id": face.id,
                    "assetId": face.asset_id,
                    "personId": face.person_id,
                    "imageWidth": face.image_width,
                    "imageHeight": face.image_height,
                    "boundingBoxX1": face.x1,
                    "boundingBoxY1": face.y1,
                    "boundingBoxX2": face.x2,
                    "boundingBoxY2": face.y2,
                    "sourceType": face.source_type,
                    "deletedAt": face.deleted_at,
                    "isVisible": face.is_visible != 0,
                }),
                &face.update_id,
            ));
        }
        Ok(())
    }

    async fn changed_users(
        &self,
        after: Option<&str>,
        before: &str,
    ) -> Result<Vec<UserSyncRow>, DbError> {
        let rows = sqlx::query_as::<_, UserSyncRow>(
            "SELECT id, name, email, avatarColor AS avatar_color, deletedAt AS deleted_at,
                    profileImagePath AS profile_image_path, profileChangedAt AS profile_changed_at,
                    isAdmin AS is_admin, pinCode AS pin_code, oauthId AS oauth_id,
                    storageLabel AS storage_label, quotaSizeInBytes AS quota_size_in_bytes,
                    quotaUsageInBytes AS quota_usage_in_bytes, updateId AS update_id
             FROM user
             WHERE (? IS NULL OR updateId > ?) AND updateId < ?
             ORDER BY updateId",
        )
        .bind(after)
        .bind(after)
        .bind(before)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    async fn user_sync_row(
        &self,
        user_id: &str,
        after: Option<&str>,
        before: &str,
    ) -> Result<Option<UserSyncRow>, DbError> {
        let row = sqlx::query_as::<_, UserSyncRow>(
            "SELECT id, name, email, avatarColor AS avatar_color, deletedAt AS deleted_at,
                    profileImagePath AS profile_image_path, profileChangedAt AS profile_changed_at,
                    isAdmin AS is_admin, pinCode AS pin_code, oauthId AS oauth_id,
                    storageLabel AS storage_label, quotaSizeInBytes AS quota_size_in_bytes,
                    quotaUsageInBytes AS quota_usage_in_bytes, updateId AS update_id
             FROM user
             WHERE id = ? AND (? IS NULL OR updateId > ?) AND updateId < ?",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .bind(before)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    async fn audit_ids(
        &self,
        sql: &str,
        user_id: &str,
        after: Option<&str>,
        before: &str,
    ) -> Result<Vec<(String, String)>, DbError> {
        let rows = sqlx::query_as(sql)
            .bind(user_id)
            .bind(after)
            .bind(after)
            .bind(before)
            .fetch_all(self.pool())
            .await?;
        Ok(rows)
    }
}

#[derive(sqlx::FromRow)]
struct SyncMemory {
    update_id: String,
    id: String,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    owner_id: String,
    kind: String,
    data: String,
    is_saved: i64,
    memory_at: String,
    seen_at: Option<String>,
    show_at: Option<String>,
    hide_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SyncAlbum {
    update_id: String,
    id: String,
    name: String,
    description: String,
    created_at: String,
    updated_at: String,
    thumbnail_asset_id: Option<String>,
    is_activity_enabled: i64,
    album_order: String,
}

#[derive(sqlx::FromRow)]
struct UserSyncRow {
    id: String,
    name: String,
    email: String,
    avatar_color: Option<String>,
    deleted_at: Option<String>,
    profile_image_path: String,
    profile_changed_at: String,
    is_admin: i64,
    pin_code: Option<String>,
    oauth_id: Option<String>,
    storage_label: Option<String>,
    quota_size_in_bytes: Option<i64>,
    quota_usage_in_bytes: i64,
    update_id: String,
}

fn user_sync_json(user: &UserSyncRow, auth: bool) -> Value {
    let color = user
        .avatar_color
        .clone()
        .unwrap_or_else(|| avatar_color_for_email(&user.email).to_string());
    let mut data = json!({
        "id": user.id,
        "name": user.name,
        "email": user.email,
        "avatarColor": color,
        "deletedAt": user.deleted_at,
        "hasProfileImage": !user.profile_image_path.is_empty(),
        "profileChangedAt": user.profile_changed_at,
    });
    if auth {
        data["isAdmin"] = json!(user.is_admin != 0);
        data["pinCode"] = json!(user.pin_code);
        data["oauthId"] = json!(user.oauth_id);
        data["storageLabel"] = json!(user.storage_label);
        data["quotaSizeInBytes"] = json!(user.quota_size_in_bytes);
        data["quotaUsageInBytes"] = json!(user.quota_usage_in_bytes);
    }
    data
}

const FACE_OWNER: &str = "
    (asset.ownerId = ? OR (
        ? = 1
        AND owner.clusterGroupId IS NOT NULL
        AND owner.clusterGroupId = (SELECT clusterGroupId FROM user WHERE id = ?)
    ))
";

#[derive(sqlx::FromRow)]
struct SyncPerson {
    update_id: String,
    id: String,
    created_at: String,
    updated_at: String,
    owner_id: String,
    name: String,
    birth_date: Option<String>,
    is_hidden: i64,
    is_favorite: i64,
    color: Option<String>,
    face_asset_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SyncFace {
    update_id: String,
    id: String,
    asset_id: String,
    person_id: Option<String>,
    image_width: i64,
    image_height: i64,
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
    source_type: String,
    is_visible: i64,
    deleted_at: Option<String>,
}

fn sync_birth(value: Option<String>) -> Value {
    match value {
        None => Value::Null,
        Some(value) if value.contains('T') => Value::String(value),
        Some(value) => Value::String(format!("{value}T00:00:00.000Z")),
    }
}

fn sync_line(kind: &str, data: Value, update_id: &str) -> String {
    format!(
        "{}\n",
        json!({ "type": kind, "data": data, "ack": format!("{kind}|{update_id}") })
    )
}

fn cursor<'a>(checkpoints: &'a HashMap<String, String>, kind: &str) -> Option<&'a str> {
    checkpoints
        .get(kind)
        .and_then(|ack| ack.split_once('|'))
        .map(|(_, update_id)| update_id)
}

fn sync_is_stale(checkpoints: &HashMap<String, String>) -> bool {
    let Some(update_id) = cursor(checkpoints, "SyncCompleteV1") else {
        return false;
    };
    let Some(millis) = uuid_v7_millis(update_id) else {
        return false;
    };
    let now = chrono::Utc::now().timestamp_millis();
    now.saturating_sub(millis) > FULL_SYNC_MS
}

fn uuid_v7_millis(id: &str) -> Option<i64> {
    let hex: String = id
        .chars()
        .filter(|character| *character != '-')
        .take(12)
        .collect();
    i64::from_str_radix(&hex, 16).ok()
}

fn is_entity_type(kind: &str) -> bool {
    matches!(
        kind,
        "AuthUserV1"
            | "AuthUserV2"
            | "UserV1"
            | "UserDeleteV1"
            | "AssetV1"
            | "AssetV2"
            | "AssetDeleteV1"
            | "AssetExifV1"
            | "AssetEditV1"
            | "AssetEditDeleteV1"
            | "AssetMetadataV1"
            | "AssetMetadataDeleteV1"
            | "AssetOcrV1"
            | "AssetOcrDeleteV1"
            | "PartnerV1"
            | "PartnerDeleteV1"
            | "PartnerAssetV1"
            | "PartnerAssetV2"
            | "PartnerAssetBackfillV1"
            | "PartnerAssetBackfillV2"
            | "PartnerAssetDeleteV1"
            | "PartnerAssetExifV1"
            | "PartnerAssetExifBackfillV1"
            | "PartnerStackBackfillV1"
            | "PartnerStackDeleteV1"
            | "PartnerStackV1"
            | "AlbumV1"
            | "AlbumV2"
            | "AlbumDeleteV1"
            | "AlbumUserV1"
            | "AlbumUserBackfillV1"
            | "AlbumUserDeleteV1"
            | "AlbumAssetCreateV1"
            | "AlbumAssetCreateV2"
            | "AlbumAssetUpdateV1"
            | "AlbumAssetUpdateV2"
            | "AlbumAssetBackfillV1"
            | "AlbumAssetBackfillV2"
            | "AlbumAssetExifCreateV1"
            | "AlbumAssetExifUpdateV1"
            | "AlbumAssetExifBackfillV1"
            | "AlbumToAssetV1"
            | "AlbumToAssetDeleteV1"
            | "AlbumToAssetBackfillV1"
            | "MemoryV1"
            | "MemoryV2"
            | "MemoryDeleteV1"
            | "MemoryToAssetV1"
            | "MemoryToAssetV2"
            | "MemoryToAssetDeleteV1"
            | "StackV1"
            | "StackDeleteV1"
            | "PersonV1"
            | "PersonDeleteV1"
            | "AssetFaceV1"
            | "AssetFaceV2"
            | "AssetFaceV3"
            | "AssetFaceDeleteV1"
            | "UserMetadataV1"
            | "UserMetadataDeleteV1"
            | "SyncAckV1"
            | "SyncResetV1"
            | "SyncCompleteV1"
    )
}
