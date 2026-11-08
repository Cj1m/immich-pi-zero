# Decide which user, session, and system-metadata columns this server stores

Type: grilling
Status: resolved

## Question

The auth and admin responses already name most of the user and session fields they read. [Decide how an operator sets the machine-learning URL and object-storage credentials](40-decide-how-an-operator-sets-the-machine-learning-url-and-object-storage-credentials.md) stores the admin config diff under system metadata key `system-config`. User onboarding and server `isOnboarded` are metadata rows, not columns on the user. The column list is in [Remaining to close](../../../docs/research/remaining-to-close.md). [Decide which user update, delete, and restore responses this server matches](41-decide-which-user-update-delete-and-restore-responses-this-server-matches.md) writes `pinCode`, `deletedAt`, and `status`. [Decide whether the web UI signs in with OAuth](49-decide-whether-the-web-ui-signs-in-with-oauth.md) stores `user.oauthId`, and `session.oauthSid` and `session.oauthBearerToken`. v3.3.6 stores these rows in Postgres. This server stores them in SQLite. Which user, session, and system-metadata columns does this server keep?

## Comments

[Decide which user update, delete, and restore responses this server matches](41-decide-which-user-update-delete-and-restore-responses-this-server-matches.md) now writes `pinCode`, `deletedAt`, and `status`. This ticket is unblocked.

Recommendation accepted: keep the v3.3.6 user, session, and metadata columns. `clusterGroupId` may be null, as [Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md) already decided. JSON values are text.

## Answer

`user` keeps `id`, `email`, `password`, `pinCode`, `createdAt`, `profileImagePath`, `isAdmin`, `shouldChangePassword`, `avatarColor`, `deletedAt`, `oauthId`, `updatedAt`, `storageLabel`, `name`, `quotaSizeInBytes`, `quotaUsageInBytes`, `status`, `profileChangedAt`, `updateId`, and `clusterGroupId`.

`email` is unique. `password`, `pinCode`, `oauthId`, `storageLabel`, `avatarColor`, `deletedAt`, and `quotaSizeInBytes` are nullable. `storageLabel` is unique. `name` and `profileImagePath` default to empty. `isAdmin` defaults to false. `shouldChangePassword` defaults to true. `quotaUsageInBytes` defaults to 0. `status` defaults to `active`. `clusterGroupId` may be null. This server still does not add a cluster. Timestamps are text. The quota figures are integers.

`session` keeps `id`, `token`, `createdAt`, `updatedAt`, `expiresAt`, `userId`, `parentId`, `deviceType`, `deviceOS`, `appVersion`, `updateId`, `isPendingSyncReset`, `pinExpiresAt`, `oauthSid`, and `oauthBearerToken`.

`token` is the SHA-256 blob. `deviceType` and `deviceOS` default to empty. `appVersion`, `expiresAt`, `parentId`, `pinExpiresAt`, `oauthSid`, and `oauthBearerToken` are nullable. `isPendingSyncReset` defaults to false. `oauthSid` and `oauthBearerToken` are what [Decide whether the web UI signs in with OAuth](49-decide-whether-the-web-ui-signs-in-with-oauth.md) stores.

`user_metadata` keeps `userId`, `key`, `value`, `updateId`, and `updatedAt`. `userId` and `key` are the primary key. `value` is JSON text. The keys already written are `onboarding` and `preferences`.

`system_metadata` keeps `key` and `value`. `value` is JSON text. The keys already written are `system-config`, `admin-onboarding`, and `MediaLocation`.
