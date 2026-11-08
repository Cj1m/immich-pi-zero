# Decide which PIN and locked-folder responses this server matches

Type: grilling
Status: resolved

## Question

The locked folder and the PIN settings call `GET /api/auth/status`, `POST /api/auth/pin-code`, `PUT /api/auth/pin-code`, `DELETE /api/auth/pin-code`, `POST /api/auth/session/lock`, and `POST /api/auth/session/unlock`. An admin PIN reset is already the user update. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do these PIN and lock calls match v3.3.6?

## Comments

Recommendation accepted: these calls match v3.3.6. A PIN is a bcrypt hash of six digits on the user row from [Decide which user, session, and system-metadata columns this server stores](issues/53-decide-which-user-session-and-system-metadata-columns-this-server-stores.md). Unlock sets `pinExpiresAt` fifteen minutes ahead. Lock clears it. Elevated permission is that expiry, which [Decide which signed-in timeline responses this server matches](issues/58-decide-which-signed-in-timeline-responses-this-server-matches.md) already requires for a locked timeline. An admin clearing the PIN stays on [Decide which user update, delete, and restore responses this server matches](issues/41-decide-which-user-update-delete-and-restore-responses-this-server-matches.md). There is no Pi difference.

## Answer

The hash uses bcrypt with ten salt rounds. The body schema accepts exactly six digits (`/^\d{6}$/`). A value that is not six digits is a 400 validation error. The swagger text that says four to six digits is not the check. The hash lives in `user.pinCode`. No new column.

`GET /api/auth/status` returns 200 `{pinCode, password, isElevated, expiresAt, pinExpiresAt}`. `pinCode` and `password` are booleans for whether those hashes are set. `isElevated` is true when this session's `pinExpiresAt` is in the future. `expiresAt` and `pinExpiresAt` are ISO strings, and they are omitted when this call has no session. The locked folder reads `isElevated` and `pinCode`. PIN settings read `pinCode`. An API key has no session, so `isElevated` is false and the two expiry fields are absent.

While a request is authenticated, a session whose `pinExpiresAt` is still in the future and falls inside the next five minutes has that expiry moved to five minutes from now.

`POST /api/auth/pin-code` returns 204 and stores the hash. A PIN already set is 400 `User already has a PIN code`. A missing user is 401.

`PUT /api/auth/pin-code` returns 204. The body is the current PIN or the password, plus `newPinCode`. The new hash replaces the old one. Other sessions stay elevated.

`DELETE /api/auth/pin-code` returns 204. The body is the current PIN or the password. `pinCode` becomes null, and every session for that user has `pinExpiresAt` cleared.

Change, reset, and unlock share these checks. No PIN is 400 `User does not have a PIN code`. A bad password is 400 `Wrong password`. A bad PIN is 400 `Wrong PIN code`. Neither secret is 400 `Either password or pinCode is required`. Password is checked before PIN when both are sent.

`POST /api/auth/session/unlock` returns 204. The body is the PIN. `pinExpiresAt` becomes fifteen minutes from now. `POST /api/auth/session/lock` returns 204 and sets `pinExpiresAt` to null on this session only. Either call without a session token is 400 `This endpoint can only be used with a session token`.

A locked timeline without elevation stays 401 `Elevated permission is required`. Setting visibility to locked does not require a PIN. Unsigned is 401. A shared link is 403 `Forbidden`. Setting a locked asset does not write a sync delete. The next status read shows the new `pinCode` and `pinExpiresAt`.
