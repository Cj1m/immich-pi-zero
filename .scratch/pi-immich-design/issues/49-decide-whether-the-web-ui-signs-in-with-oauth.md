# Decide whether the web UI signs in with OAuth

Type: grilling
Status: resolved

## Question

[Decide whether OAuth is in the Contract](09-decide-whether-oauth-is-in-the-contract.md) keeps the mobile success path on password login. [Decide how an operator sets the machine-learning URL and object-storage credentials](40-decide-how-an-operator-sets-the-machine-learning-url-and-object-storage-credentials.md) stores `oauth.enabled` on the admin config document, and a saved value is what `GET /api/server/features` and `GET /api/public/config` report. The web login page, when that public flag is true, calls `POST /api/oauth/authorize` and `POST /api/oauth/callback`. Link and unlink are `POST /api/oauth/link` and `POST /api/oauth/unlink`. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Does this server answer those four calls the way v3.3.6 does?

## Comments

Recommendation accepted: the four calls match v3.3.6. `oauth.enabled` defaults to false, so an unconfigured server still shows password login. The Contract success path stays password.

## Answer

The four calls match v3.3.6. The Pi can redirect to an issuer. There is no Pi difference.

`oauth.enabled` defaults to false. `GET /api/server/features` reports `oauth` and `oauthAutoLaunch` from `oauth.enabled` and `oauth.autoLaunch`. `GET /api/public/config` includes `oauth.enabled`, `oauth.autoLaunch`, and `oauth.buttonText`, and `passwordLogin.enabled`. The login page reads those. While `oauth.enabled` is false the page shows email and password and does not call these routes. That is the unconfigured server from [Decide whether OAuth is in the Contract](09-decide-whether-oauth-is-in-the-contract.md). The Contract success path stays password. When an operator turns `oauth.enabled` on, the same flag is what the phone reads, and the phone uses these same two login calls with redirect `app.immich:///oauth-callback`.

`POST /api/oauth/authorize` with `{ redirectUri }` returns 201 `{ url }`. The page reads `url` and navigates there. The response also sets `immich_oauth_state` and `immich_oauth_code_verifier`, both HttpOnly, Path `/`, SameSite Lax, Max-Age 400 days, Secure on HTTPS. When `mobileOverrideEnabled` is set, a redirect of `app.immich:///oauth-callback` is replaced with `mobileRedirectUri`. Disabled OAuth is 400 `OAuth is not enabled`.

`POST /api/oauth/callback` with `{ url }` returns 201 and the same login document password login returns. The page reads `isOnboarded`. The response sets `immich_access_token` and `immich_auth_type` to `oauth`, and `immich_is_authenticated` to `true`. That last cookie is not HttpOnly. The state and verifier cookies are cleared. The Session stores `oauthSid` and `oauthBearerToken` from the issuer, and the User stores `oauthId`. A matching email with no `oauthId` links that User. A matching email that already has a different `oauthId` is 400 `OAuth authentication failed`. No User, and `autoRegister` false, is the same 400. No email on a new User is 400 `OAuth profile does not have an email address`. Missing state is 400 `OAuth state is missing`. Missing verifier is 400 `OAuth code verifier is missing`. Disabled OAuth is 400 `OAuth is not enabled`. `autoRegister` defaults to true.

`POST /api/oauth/link` with `{ url }` returns 200 and the admin user document. The settings page reads `oauthId`. A `sub` already linked to another User is 400 `This OAuth account has already been linked to another user.` The same state and verifier rules apply. The Session stores `oauthSid` and `oauthBearerToken` when the issuer sends them.

`POST /api/oauth/unlink` returns 200 and the admin user document with `oauthId` empty. The Session clears `oauthSid` and `oauthBearerToken`.

Logout of an OAuth Session returns `redirectUri` for the end-session endpoint, with `id_token_hint` when `oauthBearerToken` is stored. The web navigates there. With OAuth off, or with no endpoint, that URI is `/auth/login?autoLaunch=0`.

`GET /api/oauth/mobile-redirect` is public and 307 to `app.immich:///oauth-callback` plus the query string. `POST /api/oauth/backchannel-logout` with form field `logout_token` is public and 200 with an empty body. It drops the Sessions for that `sid` or `sub`. Disabled OAuth is 400 `Received backchannel logout request but OAuth is not enabled`. A token that fails validation is 400 `Error backchannel logout: token validation failed`. No claims is 400 `Invalid logout token: no claims found`. A token with neither `sub` nor `sid` is 400 `Invalid logout token: it must contain either a sub or a sid claim`.

The issuer, client id, client secret, and the other `oauth.*` keys stay on the admin config document.
