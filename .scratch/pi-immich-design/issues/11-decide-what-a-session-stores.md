# Decide what a session stores

Type: grilling
Status: resolved
Blocked by: 09

## Question

The app keeps a session as the `immich_access_token` cookie, with `immich_auth_type` and `immich_is_authenticated`. It checks that cookie with `POST /api/auth/validateToken`. It does not send a bearer header and it does not refresh the session. [Decide whether OAuth is in the Contract](09-decide-whether-oauth-is-in-the-contract.md) left password login as the only sign-in path. What must the server store for that cookie so login, validateToken, and logout match the app?

## Comments

Recommendation accepted: a password login creates a Session. The server stores the SHA-256 of `immich_access_token` and the User it belongs to. Logout deletes that Session, so a later `validateToken` with the old cookie is unauthorized. A second client has its own Session. The other two cookies are fixed flags the server sets and clears.

Recommendation accepted: a Session stays valid until logout. Months without a request do not end it. There is no idle cutoff. A later sign-in adds a Session and leaves the previous one in place.

Recommendation accepted: login sets the three cookies with `Max-Age` 400 days, path `/`, `SameSite=Lax`, and `Secure` only when the login request was HTTPS. `HttpOnly` is set on `immich_access_token` and `immich_auth_type`, and not on `immich_is_authenticated`. Logout clears all three. The Session row remains after the phone drops the cookie.

Recommendation accepted: the server honors a Session only from the `immich_access_token` cookie. `Authorization: Bearer`, `x-immich-user-token`, and `x-immich-session-token` are not accepted.

## Answer

A password login creates a Session. The server stores the SHA-256 of an opaque token and the User it belongs to. It does not store the plaintext, a device, an expiry, a PIN, or anything about OAuth.

That plaintext is the `accessToken` in the login JSON and the value of the `immich_access_token` cookie. `immich_auth_type` is `password` and `immich_is_authenticated` is `true`. Those two are fixed flags. The server sets and clears them with the token and does not look them up.

All three cookies use path `/`, `SameSite=Lax`, and `Max-Age` 400 days. `Secure` is set only when the login request was HTTPS. `HttpOnly` is set on `immich_access_token` and `immich_auth_type`, and not on `immich_is_authenticated`.

A Session lasts until logout. A quiet stretch does not end it. A later sign-in creates another Session and leaves the earlier one in place. After 400 days the phone drops the cookie. The row remains, because that phone can no longer present it.

The server reads a Session only from `immich_access_token`. `Authorization: Bearer`, `x-immich-user-token`, and `x-immich-session-token` are not accepted.

`POST /api/auth/validateToken` hashes the cookie. A matching Session returns `{ authStatus: true }`. A missing or unknown cookie is unauthorized, and the app returns to the login form.

`POST /api/auth/logout` deletes that Session and clears the three cookies. Any other Session for that User stays. The body is `{ "successful": true, "redirectUri": "/auth/login?autoLaunch=0" }`. The app does not read those two fields. Both are present so the response parses.

The login JSON also carries `isAdmin`, `name`, `profileImagePath`, `shouldChangePassword`, `userEmail`, and `userId` from the User. Those fields are not stored on the Session. The success status is 201.

## Comments

[Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md) amends the "no device" line. A Session also stores `deviceType`, `deviceOS`, `appVersion`, `createdAt`, and `updatedAt`, so the admin detail page can draw the device list. The login JSON also includes `isOnboarded`.

[Decide which user update, delete, and restore responses this server matches](41-decide-which-user-update-delete-and-restore-responses-this-server-matches.md) amends "lasts until logout." A soft-deleted User's Session row stays, and the cookie stops validating until restore. The hard-delete job removes the row with the User.
