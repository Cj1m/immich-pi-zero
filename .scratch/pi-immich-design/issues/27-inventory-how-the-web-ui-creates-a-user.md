# Inventory how the web UI creates a User

Type: research
Status: resolved

## Question

[Decide how a later User is created](20-decide-how-a-later-user-is-created.md) has the admin create a later User from the new-user screen. That screen sits on the user list, and a successful create opens the User's detail page. Which HTTP calls does the v3.3.6 web UI make for that path? Include `GET /api/admin/users`, `POST /api/admin/users`, the preferences, statistics, and sessions reads, and the upload heatmap. Name the method, path, and the fields each page requires. Say what a missing field does to the list and to the detail page. Also include `POST /api/auth/change-password` and `PUT /api/users/me/onboarding`, which that decision requires before a non-admin reaches the library on the web. The phone's change-password call is already noted in `docs/research/login-and-server-info.md`. Write the note to `docs/research/later-user.md` and link it from this ticket when resolving.

## Answer

The list is `GET /api/admin/users?withDeleted=true`. Create is `POST /api/admin/users`. The detail page then loads preferences, statistics, and sessions, and draws the upload heatmap from `GET /api/admin/users/{id}/calendar-heatmap`. A missing list field blanks a cell or draws the infinity icon. A missing nested preference throws. A thrown list call does not draw the table. A thrown detail search redirects to the list. A thrown preferences, statistics, or sessions call does not draw the detail page.

The web screen a non-admin hits before the library is `PUT /api/users/me` with the new password, then `POST /api/auth/logout`. It does not call `POST /api/auth/change-password`. That POST is the user-settings form, and it requires a new password of at least 8 characters. The PUT does not. Onboarding ends with `PUT /api/users/me/onboarding`. The privacy step also sends `PUT /api/users/me/preferences`.

Per-call fields are in [Later user](../../../docs/research/later-user.md).
