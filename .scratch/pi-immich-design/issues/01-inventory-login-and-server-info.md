# Inventory login and server info

Type: research
Status: resolved

## Question

Which calls does the official mobile app in `reference/immich` (tag v3.3.6) make to log in, keep a session, and pass the server checks it runs before showing the library? Cover ping, version, config, and feature flags, and any auth call the spec's auth list does not name. For each call, record the method, path, request shape, and the response fields the app requires. Write the inventory to `docs/research/login-and-server-info.md` and link it from this ticket when resolving.

## Answer

The app at v3.3.6 reaches the timeline without creating a User. Per-call fields are in [Login and server info](../../../docs/research/login-and-server-info.md).

The URL check is `GET /.well-known/immich` (failure ignored) then `GET /api/server/ping` (must succeed). Then `GET /api/server/version`, `GET /api/server/features`, and `GET /api/server/config`. Those three do not block the timeline. Before the form, `passwordLogin` and `oauth` choose the buttons, and `oauthButtonText` labels OAuth. A version mismatch is a warning.

Password login is `POST /api/auth/login` with `email` and `password`. OAuth, when the feature flag says so, is `POST /api/oauth/authorize` then `POST /api/oauth/callback`. The app never calls `POST /api/auth/admin-sign-up`.

The session is the `immich_access_token` cookie, with `immich_auth_type` and `immich_is_authenticated`. The app does not send `Authorization: Bearer` and has no refresh call. `POST /api/auth/validateToken` runs after the timeline shell is allowed and returns the user to login on 401 or when `authStatus` is not true. `GET /api/users/me` and `GET /api/users/me/preferences` run after the token is stored. A failure there does not, by itself, stop password login from opening the timeline.
