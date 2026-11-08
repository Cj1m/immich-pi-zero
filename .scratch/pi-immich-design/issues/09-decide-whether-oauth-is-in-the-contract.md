# Decide whether OAuth is in the Contract

Type: grilling
Status: resolved

## Question

The login inventory shows two ways the official app reaches the timeline. Password login is `POST /api/auth/login`. When server features report `oauth`, the app instead uses `POST /api/oauth/authorize` and `POST /api/oauth/callback`. The spec's auth list does not name OAuth. Is OAuth login part of the Contract for this Pi server, or is password login the only sign-in path?

## Comments

Recommendation accepted: password login is the only sign-in path. The server reports `passwordLogin: true` and `oauth: false`, and it does not implement the authorize or callback routes.

[Decide whether the web UI signs in with OAuth](49-decide-whether-the-web-ui-signs-in-with-oauth.md) amends the flag and the routes. `oauth` follows `oauth.enabled`, which defaults to false, so this unconfigured server still shows password login. The routes exist for the web UI when an operator turns that flag on. The Contract success path stays password.

## Answer

Password login is the only sign-in path in the Contract. The app reaches the timeline through `POST /api/auth/login`.

The server reports `passwordLogin: true` and `oauth: false` on `GET /api/server/features`. The login form then shows email and password and does not show the OAuth button. `POST /api/oauth/authorize` and `POST /api/oauth/callback` are not part of the Contract, and neither is an external identity provider.

A features response the app cannot parse leaves the same form: password on, OAuth off. That default is not a second sign-in path.
