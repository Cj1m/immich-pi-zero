# Decide how a later User is created

Type: grilling
Status: resolved

## Question

The app never creates a User. [Decide how the first User is created](10-decide-how-the-first-user-is-created.md) seeds the admin from config when the server has no User, and `POST /api/auth/admin-sign-up` is not on this server. A User can still be included on an album or a shared link. How does a User after the first one come into existence?

## Comments

Recommendation accepted: the admin creates each later User from the official web UI's new-user screen. The server accepts `POST /api/admin/users`, stores the email, the name, and the password hash, and makes the User an admin only when the request sets `isAdmin`. Startup config still creates only the first User. The phone still never creates a User.

Recommendation accepted: store `shouldChangePassword` from the request, and store true when the field is omitted. Login returns it. A non-admin with the flag set reaches the library only after `POST /api/auth/change-password` succeeds with the current password and a new password of at least 8 characters. That call stores the new hash and clears the flag. The phone then logs out, and the next login opens the library. An admin skips that screen even when the flag is set.

Recommendation accepted: this server does not send the welcome message. `GET /api/server/features` reports `email: false`, so the new-user screen hides the welcome-email switch. A create with `notify: true` still returns the new User and sends nothing. The admin tells that User the password.

Recommendation accepted: a later User has a storage quota. Null or an omitted `quotaSizeInBytes` means no cap. A number is the cap in bytes. An upload that would pass the cap returns `400` with `Quota has been exceeded!` and keeps nothing. The usage the phone shows is the bytes that User has stored. The admin created from config stays uncapped.

Recommendation accepted: a later User is created with no onboarding row. Login returns `isOnboarded: false`. The phone still opens the library once any required password change is done. The web UI shows the user onboarding pages, and `PUT /api/users/me/onboarding` with `{ "isOnboarded": true }` stores that flag. The server-wide admin onboarding flag is unchanged.

Recommendation accepted: an admin's session can read the user list, including the new User, and the detail page renders for that User: default preferences, zero asset counts, an empty session list, and an empty upload heatmap. Changing, deleting, restoring, or resetting that User waits with the rest of the web UI.

Recommendation accepted: the new User is a `UsersV1` row. After sync, invite-people lists them, and adding them puts that id on the album as an editor. A shared link does not require an account.

## Answer

The admin creates each later User from the official web UI's new-user screen. `POST /api/admin/users` stores the email, the name, and the password hash. The User is an admin only when the request sets `isAdmin`. Startup config still creates only the first User, and that admin stays uncapped. The phone never creates a User. A session that is not an admin cannot create one.

`shouldChangePassword` is stored from the request, and stored as true when the field is omitted. Login returns it. A non-admin with the flag set reaches the library only after `POST /api/auth/change-password` succeeds with the current password and a new password of at least 8 characters. That call stores the new hash and clears the flag. The phone then logs out, and the next login opens the library. An admin skips that screen even when the flag is set.

No welcome mail is sent. `GET /api/server/features` reports `email: false`, so the new-user screen hides the welcome-email switch. A create with `notify: true` still returns the User. The admin tells that User the password.

A null or omitted `quotaSizeInBytes` means no cap. A number is the cap in bytes. An upload that would pass it returns `400` with `Quota has been exceeded!` and keeps nothing. The usage the phone shows is the bytes of that User's Originals.

The User is created with no onboarding row, so login returns `isOnboarded: false`. The phone does not read that flag and still opens the library once any required password change is done. The web UI shows the user onboarding pages (hello, theme, language, privacy, and the mobile-app note). `PUT /api/users/me/onboarding` with `{ "isOnboarded": true }` stores the flag. The server-wide admin onboarding flag stays as it is.

An admin's session can load the user list with `GET /api/admin/users?withDeleted=true` and open `/admin/users/{id}` for the new User. That page gets the User, default preferences, zero asset counts, an empty session list, and an empty upload heatmap. Changing, deleting, restoring, or resetting that User waits with the rest of the web UI.

The new User is a `UsersV1` sync row: id, name, email, avatar color, `deletedAt`, whether a profile image exists, and `profileChangedAt`. A missing avatar color is shown as `primary`. After sync, invite-people lists them. `PUT /api/albums/{id}/users` adds that id as an editor. An id that is not a User returns `400` with `Invalid user`. A shared link stays a URL. Opening one does not create a User.

A duplicate email returns `400` with `Email is not available` and creates nothing. With password login as the only sign-in path, a missing password returns `400` with `password is required`.

The calls the new-user screen, the detail page, the change-password screen, and web onboarding require are [Inventory how the web UI creates a User](27-inventory-how-the-web-ui-creates-a-user.md).

## Comments

[Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md) keeps the phone on `POST /api/auth/change-password`. The web page before the library is `PUT /api/users/me` with the new password only. Server `isOnboarded` starts true, so the server wizard does not run.

[Decide which user update, delete, and restore responses this server matches](41-decide-which-user-update-delete-and-restore-responses-this-server-matches.md) decides the change, delete, restore, and reset that this answer left open.
