# Later user

Which HTTP calls the official web UI makes when an admin opens the user list, opens the new-user screen, creates a User, and lands on that User's detail page. Also the calls a non-admin makes on the web change-password screen and the user onboarding pages before the library. Read from `reference/immich` at tag `v3.3.6`, commit `587f549f46f1cd1822a06e16dcccecef04ea4565`. This is the web UI. The phone's change-password call is already in [Login and server info](login-and-server-info.md). The phone logs out after `POST /api/auth/change-password`. This note does not re-inventory that Dart client.

The web app is client-only (`reference/immich/web/src/routes/+layout.ts:9-10`). `/admin/users`, `/admin/users/new`, and `/admin/users/{id}` are client routes after the document boots. The web SDK does not check the JSON against the schema. A missing field throws only when the page reads it off `null` or `undefined`, or when `oazapfts.ok` rejects the status. The generated client accepts only the status it declares. Paths below start with `/api` because the SDK `baseUrl` is `/api` (`reference/immich/packages/sdk/src/fetch-client.ts:11`).

Changing, deleting, restoring, or resetting a User is not this path. The list's context menu can open those actions. A successful create does not. `GET /api/admin/users/{id}` (`getUserAdmin`) is not called. The detail page loads the User through the search route.

## Sequence

**Boot.** Root `init` loads a bundled locale, then `GET /api/server/config`, then `authManager.load`, then `GET /api/server/features` unless `maintenanceMode` is true (`reference/immich/web/src/lib/utils/server.ts:10-21`). A browser with the `immich_is_authenticated` cookie loads `GET /api/users/me` and `GET /api/users/me/preferences` together. If `user.license.activatedAt` is missing it also calls `GET /api/server/about`. A failure of that pair is ignored and the user stays signed out (`reference/immich/web/src/lib/managers/auth-manager.svelte.ts:50-81`).

**Admin shell.** `/admin/users` requires an admin session. `authenticate` redirects a signed-out browser to `/auth/login`, and a signed-in non-admin to `/photos` (`reference/immich/web/src/lib/utils/auth.ts:13-27`). The admin layout then calls `GET /api/system-config` and `GET /api/system-config/defaults` (`reference/immich/web/src/routes/admin/+layout.ts:5-7`, `reference/immich/web/src/lib/managers/system-config-manager.svelte.ts:15-18`). The user list, the new-user form, and the detail page do not read that body. A thrown config call still fails the layout, so those pages do not render.

**List.** The list layout calls `GET /api/server/storage`, then `GET /api/admin/users?withDeleted=true` (`reference/immich/web/src/routes/admin/users/(list)/+layout.ts:6-9`, `reference/immich/web/src/lib/utils/auth.ts:30-36`). The table has no search box. It draws every returned row.

**New user.** `/admin/users/new` is a modal over that list. Its own load only repeats `authenticate` (`reference/immich/web/src/routes/admin/users/(list)/new/+page.ts:5-7`). Submit is `POST /api/admin/users`. Success navigates to `/admin/users/{id}` (`reference/immich/web/src/routes/admin/users/(list)/new/+page.svelte:46-57`). The create event also refetches the list when the new id is not already in the table (`reference/immich/web/src/routes/admin/users/(list)/+layout.svelte:37-43`, `65-67`).

**Detail.** The id must match the UUID pattern or the load redirects to the list before any user call (`reference/immich/web/src/routes/admin/users/[id]/+layout.ts:13-15`, `reference/immich/web/src/lib/constants.ts:1`). Otherwise `GET /api/admin/users?id={id}&withDeleted=true`. An empty list, or a thrown search, redirects to the list (`+layout.ts:17-20`). Then, together: `GET /api/admin/users/{id}/preferences`, `GET /api/admin/users/{id}/statistics`, and `GET /api/admin/users/{id}/sessions`. The upload heatmap is a separate `GET /api/admin/users/{id}/calendar-heatmap` after the page renders (`reference/immich/web/src/routes/admin/users/[id]/+layout.svelte:221`). A non-empty `profileImagePath` also requests `GET /api/users/{id}/profile-image`.

**A later User signing in on the web.** The login page calls `GET /api/public/config`. Password submit is `POST /api/auth/login`. A non-admin with `shouldChangePassword` goes to `/auth/change-password` and does not open the library (`reference/immich/web/src/routes/auth/login/+page.svelte:84-95`). That screen submits `PUT /api/users/me` with the new password, then `POST /api/auth/logout`. It does not call `POST /api/auth/change-password`. That POST is the password form on the user-settings page, which is already inside the library (`reference/immich/web/src/routes/(user)/user-settings/ChangePasswordSettings.svelte:14`). After the next login, `isOnboarded` false opens `/auth/onboarding`. The last user step calls `PUT /api/users/me/onboarding`, then `/photos`.

## Calls

### `GET /api/server/config`

- When: root `init`, before the list and before login (`server.ts:16`).
- Method and path: `GET /api/server/config`. Public. The client accepts only 200 (`fetch-client.ts:6767-6773`, `reference/immich/server/src/controllers/server.controller.ts:114-127`).
- Request: no body.
- Response fields this path requires: `maintenanceMode` and, on the login load, `isInitialized`. `maintenanceMode` true leaves features unloaded (`server.ts:19-21`). The handler always sends `maintenanceMode: false` (`reference/immich/server/src/services/server.service.ts:130`). A missing `isInitialized` is treated as not initialized, and the login load redirects to `/auth/register` (`reference/immich/web/src/routes/auth/login/+page.ts:18-21`). A thrown config call is the root error layout (`+layout.ts:13-24`). The user list does not read the other config fields. An admin's own first login also reads `isOnboarded` on this body (`login/+page.svelte:86-88`). A later non-admin does not.
- Source: `server.ts:10-21`, `server.service.ts:113-132`, `login/+page.ts:18-21`.

### `GET /api/users/me` and `GET /api/users/me/preferences`

- When: `authManager.load`, when the `immich_is_authenticated` cookie is present (`auth-manager.svelte.ts:50-65`). Again on the change-password and onboarding loads, because those call `authenticate` (`reference/immich/web/src/routes/auth/change-password/+page.ts:8-9`, `reference/immich/web/src/routes/auth/onboarding/+page.ts:5-6`). Skipped when both objects are already in memory (`auth-manager.svelte.ts:50-52`).
- Method and path: `GET /api/users/me` and `GET /api/users/me/preferences`. The client accepts only 200 (`fetch-client.ts:7617-7623`, `7732-7738`). Both require a user session (`reference/immich/server/src/controllers/user.controller.ts:55-63`, `99-107`). The controller prefix is `users` (`user.controller.ts:37-38`, `reference/immich/server/src/enum.ts:648`).
- Request: no body.
- Response fields this path requires: both calls must succeed or the session is dropped (`auth-manager.svelte.ts:62-81`). `authenticated` is true only when both objects are set (`auth-manager.svelte.ts:24-26`). The admin check reads `user.isAdmin` (`auth.ts:25-27`). A missing `isAdmin` is not an admin, so the user list redirects to `/photos`. The change-password load reads `user.shouldChangePassword`. Missing or false redirects to `/photos` and the form never shows (`change-password/+page.ts:10-12`). The greeting reads `user.name` and `user.email` (`change-password/+page.svelte:36`). A missing name or email is an empty interpolation. It does not block the form. Onboarding reads `user.isAdmin` and `user.name` (`onboarding/+page.svelte:104`, `reference/immich/web/src/routes/auth/onboarding/OnboardingHello.svelte:16`). The privacy step reads `preferences.cast.gCastEnabled` (`reference/immich/web/src/routes/auth/onboarding/OnboardingUserPrivacy.svelte:9`). A missing `cast` throws when that step renders. If `user.license.activatedAt` is missing, `GET /api/server/about` runs and its failure is ignored (`auth-manager.svelte.ts:68-75`, `fetch-client.ts:6745-6751`). The user list does not read the about body.
- Source: `auth-manager.svelte.ts:50-81`, `auth.ts:13-27`.

### `GET /api/server/features`

- When: the same `init`, after config, when `maintenanceMode` is not true (`server.ts:19-21`). The new-user form reads it. The list table does not.
- Method and path: `GET /api/server/features`. Public. The client accepts only 200 (`fetch-client.ts:6778-6784`, `server.controller.ts:99-111`).
- Request: no body.
- Response fields this path requires: `email` and `oauth`. `email` false hides the welcome-email switch (`new/+page.svelte:74-78`). The hidden switch does not change `notify`. It stays at its initial `true`, and submit still sends `notify: true`. A missing `email` is falsy, so the switch stays hidden. `oauth` true makes the password fields not required (`new/+page.svelte:80-84`). A missing `oauth` is falsy, so they stay required. The form's submit check does not use either flag. It only rejects a password mismatch (`new/+page.svelte:29-31`). Reading `.value` before a successful features load throws (`reference/immich/web/src/lib/managers/feature-flags-manager.svelte.ts:17-22`). The server sets `email` from `notifications.smtp.enabled` and `oauth` from `oauth.enabled` (`server.service.ts:103-108`).
- Source: `server.ts:19-21`, `feature-flags-manager.svelte.ts:13-27`, `new/+page.svelte:74-84`, `server.service.ts:93-110`.

### `GET /api/system-config` and `GET /api/system-config/defaults`

- When: every admin layout load, including the user list, the new-user modal, and the detail page (`admin/+layout.ts:5-7`). Not the login, change-password, or onboarding pages.
- Method and path: `GET /api/system-config`, then `GET /api/system-config/defaults`. Both admin-only. The client accepts only 200 (`fetch-client.ts:7258-7290`, `reference/immich/server/src/controllers/system-config.controller.ts:11-45`).
- Request: no body.
- Response fields this path requires: none. The user list, the new-user form, and the detail page do not read `systemConfigManager.value`. A thrown call fails the admin layout, so those pages do not render. A missing field inside a 200 body does not change the table or the detail page.
- Source: `system-config-manager.svelte.ts:15-54`, `admin/+layout.ts:5-7`.

### `GET /api/server/storage`

- When: the list layout and the detail layout, after `authenticate`, via `requestServerInfo` (`(list)/+layout.ts:8`, `[id]/+layout.ts:11`, `auth.ts:30-36`). Skipped when there is no session.
- Method and path: `GET /api/server/storage`. The client accepts only 200 (`fetch-client.ts:6859-6865`, `server.controller.ts:55-64`).
- Request: no body.
- Response fields this path requires: `diskSizeRaw`, and only as a warning on the new-user form. The warning shows when a typed quota is greater than `diskSizeRaw` (`new/+page.svelte:24-27`). It does not disable submit. A missing `diskSizeRaw` makes that comparison false, so the warning stays hidden. A missing `serverInfo` does the same. A thrown storage call fails the list layout and the detail layout. The table and the detail page do not otherwise read this body. The server sets `diskSizeRaw` from the disk total (`server.service.ts:78`, `reference/immich/server/src/dtos/server.dto.ts:53`).
- Source: `auth.ts:30-36`, `new/+page.svelte:24-27`, `server.controller.ts:55-64`.

### `GET /api/admin/users?withDeleted=true`

- When: the list layout (`(list)/+layout.ts:9`). Again when a create, update, delete, or restore event names an id that is not already in the table (`(list)/+layout.svelte:37-43`). The detail page uses the same route with `id`. It is not `GET /api/admin/users/{id}`.
- Method and path: `GET /api/admin/users` with query `withDeleted=true`. The detail load also sends `id` set to the path id (`[id]/+layout.ts:17`, `fetch-client.ts:4198-4210`). The client accepts only 200. Admin only (`reference/immich/server/src/controllers/user-admin.controller.ts:22-35`). The server accepts the query string `true` or `false` and rejects any other string (`reference/immich/server/src/validation.ts:213-215`, `reference/immich/server/src/dtos/user.dto.ts:69-74`). `id` must be a UUID when it is sent (`user.dto.ts:72`).
- Request: no body.
- Response fields the list requires: an array. The table reads `id`, `name`, `email`, `quotaSizeInBytes`, and `deletedAt` (`(list)/+layout.svelte:85-98`). `id` is the row key and the link to `/admin/users/{id}`. A missing `name` or `email` is a blank cell. `quotaSizeInBytes` null, missing, or negative draws the infinity icon. A number that is `>= 0` is formatted as a size (`(list)/+layout.svelte:93-97`). `deletedAt` set colors the row. Missing does not. The load has no catch. A thrown call does not draw the table. A 200 body that is not an array throws when the table iterates it.
- Response fields the detail page requires: a one-element array. The load keeps the first element. Empty, or a thrown search, redirects to `/admin/users` (`[id]/+layout.ts:17-20`). The page then reads `id`, `name`, `email`, `createdAt`, `updatedAt`, `isAdmin`, `deletedAt`, `quotaSizeInBytes`, and `quotaUsageInBytes` (`[id]/+layout.svelte:58-65`, `107-164`, `183-201`). `quotaUsageInBytes` null or missing is drawn as zero. `quotaSizeInBytes` null or missing draws "unlimited". A number `>= 0` draws the meter, including zero. `isAdmin` true draws the admin badge. `deletedAt` set draws the deleted alert. `name` is the heading and the breadcrumb. The avatar reads `profileImagePath`, `avatarColor`, `name`, `email`, and `profileChangedAt` (`reference/immich/web/src/lib/components/shared-components/UserAvatar.svelte:70-88`). An empty or missing `profileImagePath` draws the first letter of `name` and does not request an image. A missing `avatarColor` throws when the color class is looked up (`UserAvatar.svelte:70`). `createdAt` and `updatedAt` are passed to `new Date` and formatted. They are not a load failure.
- The server maps each row with `mapUserAdmin` (`reference/immich/server/src/services/user-admin.service.ts:25-30`, `user.dto.ts:139-160`). `avatarColor` falls back from the email when the stored value is null (`user.dto.ts:64`). `oauthId` null is sent as `''` (`user.dto.ts:155`). The list and the detail page do not read `oauthId`, `storageLabel`, `status`, `license`, or `clusterGroupId`.
- Source: `(list)/+layout.ts:9`, `(list)/+layout.svelte:85-98`, `[id]/+layout.ts:13-26`, `[id]/+layout.svelte:107-201`, `user-admin.controller.ts:26-35`, `user.dto.ts:69-74`, `139-160`.

### `POST /api/admin/users`

- When: the new-user form submits (`new/+page.svelte:46-54`, `reference/immich/web/src/lib/services/user-admin.service.ts:100-110`).
- Method and path: `POST /api/admin/users`. The client accepts only 201 (`fetch-client.ts:4215-4225`). The handler sets no `@HttpCode`. The e2e expects 201 (`reference/immich/e2e/src/specs/server/api/user-admin.e2e-spec.ts:80-96`, `user-admin.controller.ts:37-46`).
- Request: JSON `email`, `password`, `shouldChangePassword`, `name`, `quotaSizeInBytes`, `notify`, and `isAdmin`. The form does not send `avatarColor`, `pinCode`, or `storageLabel`. Initial state is `shouldChangePassword: true`, `notify: true`, `isAdmin: false`, and quota unset (`new/+page.svelte:14-21`). An unset quota is `undefined`. `Number(undefined)` is `NaN`, and `convertToBytes` multiplies that by `1024 ** 3` (`reference/immich/web/src/lib/utils/byte-units.ts:55-57`). `JSON.stringify` writes `NaN` as `null`, so an empty quota is sent as `quotaSizeInBytes: null`. A typed `0` is sent as `0`. The client does not require a non-empty email, name, or password. `valid` is only "the two passwords match, and a create is not already running" (`new/+page.svelte:29-41`).
- Response fields the page requires: a truthy body, then `id` for `/admin/users/{id}` (`new/+page.svelte:56-57`, `reference/immich/web/src/lib/route.ts:136`). The page does not read the other response fields before navigating. A thrown call toasts the server `message`, or `errors.unable_to_create_user`, and stays on the form (`user-admin.service.ts:108-109`, `reference/immich/web/src/lib/utils/handle-error.ts:38-57`). The list does not gain a row from the response alone. The `UserAdminCreate` event refetches the list when that id is absent (`(list)/+layout.svelte:37-43`).
- Server: `email` is required and lowercased. An invalid email is 400 (`validation.ts:137-142`, `user.dto.ts:78-91`). A duplicate email is 400 `Email is not available` and creates nothing (`reference/immich/server/src/services/base.service.ts:322-327`). `password` is a required string on the schema, so a missing password is 400 before the service. An empty string passes the schema. When OAuth is disabled, a falsy password is 400 `password is required` (`user-admin.service.ts:36-38`). `shouldChangePassword` omitted uses the column default `true` (`reference/immich/server/src/schema/migrations/1744910873969-InitialMigration.ts:111`). The form always sends the boolean. `isAdmin` omitted defaults to false in that same migration. `quotaSizeInBytes` null or omitted stores null. A number must be an integer `>= 0` (`user.dto.ts:86`). `notify` is not stored on the user. The handler emits `UserSignup` with `notify: !!notify` and returns the user (`user-admin.service.ts:42-48`). The browser does not make a second call for the mail. A create while no admin exists, with `isAdmin` false, is 400 `The first registered account must the administrator.` (`base.service.ts:329-333`). That is not the later-user path. The admin already exists.
- Source: `new/+page.svelte:14-61`, `user-admin.service.ts:33-49`, `100-110`, `user.dto.ts:78-91`, `base.service.ts:322-349`.

### `GET /api/admin/users/{id}/preferences`

- When: the detail layout, in parallel with statistics and sessions, after the User row is found (`[id]/+layout.ts:22-26`). Not the list.
- Method and path: `GET /api/admin/users/{id}/preferences`. The client accepts only 200 (`fetch-client.ts:4295-4303`, `user-admin.controller.ts:145-154`).
- Request: no body. `id` is the path.
- Response fields the page requires: `emailNotifications.enabled`, `folders.enabled`, `memories.enabled`, `people.enabled`, `ratings.enabled`, `sharedLinks.enabled`, `purchase.showSupportBadge`, `tags.enabled`, and `cast.gCastEnabled` (`[id]/+layout.svelte:168-178`). A missing parent, such as `emailNotifications`, throws on `.enabled` and the features card does not render. A present parent with `enabled` missing is drawn as the off icon, because the icon treats a falsy state as off (`reference/immich/web/src/routes/admin/users/[id]/FeatureSetting.svelte:15`). One thrown call in the `Promise.all` fails the layout. The detail page does not render. The server fills a new User from defaults: email notifications on, folders off, memories on, people on, ratings off, shared links on, support badge on, tags off, cast off (`user-admin.service.ts:143-146`, `reference/immich/server/src/utils/preferences.ts:8-58`). A missing preferences row is those defaults, not an empty object.
- Source: `[id]/+layout.ts:22-26`, `[id]/+layout.svelte:168-178`, `preferences.ts:8-69`.

### `GET /api/admin/users/{id}/statistics`

- When: the same parallel load (`[id]/+layout.ts:24`).
- Method and path: `GET /api/admin/users/{id}/statistics`. No query. The client accepts only 200 (`fetch-client.ts:4351-4366`, `user-admin.controller.ts:130-143`). Optional `isFavorite`, `isTrashed`, and `visibility` are omitted, so the repository counts rows with `deletedAt` null and its default visibility (`reference/immich/server/src/repositories/asset.repository.ts:710-723`).
- Request: no body.
- Response fields the page requires: `images` and `videos` (`[id]/+layout.svelte:125-136`). The page does not read `total`. A missing `images` or `videos` is passed to the count card as `value`. The card calls `value.toString()` when measuring the width (`reference/immich/web/src/lib/components/server-statistics/ServerStatisticsCard.svelte:20-25`, `43-45`). That throw is in the render, not the promise, so the card's `{:catch}` does not take it. The server sends `images`, `videos`, and `total` (`reference/immich/server/src/dtos/asset.dto.ts:87-93`, `172-177`). A User with no assets is zero for each count the query returns. A thrown call fails the whole detail load.
- Source: `[id]/+layout.ts:22-26`, `[id]/+layout.svelte:125-136`, `user-admin.service.ts:138-140`, `asset.dto.ts:87-93`.

### `GET /api/admin/users/{id}/sessions`

- When: the same parallel load (`[id]/+layout.ts:25`).
- Method and path: `GET /api/admin/users/{id}/sessions`. The client accepts only 200 (`fetch-client.ts:4338-4346`, `user-admin.controller.ts:119-128`).
- Request: no body.
- Response fields the page requires: an array. Empty draws "no devices" (`[id]/+layout.svelte:208-212`). Not an array throws when the page iterates it. Each row reads `id` as the key, `deviceOS`, `deviceType`, `appVersion`, and `updatedAt` (`reference/immich/web/src/lib/components/user-settings-page/DeviceCard.svelte:35-76`). The detail page does not pass a delete handler, so it does not read `current`. Both `deviceOS` and `deviceType` empty draw "unknown". `deviceOS` null or missing throws on `.includes` once the equality checks fail (`DeviceCard.svelte:39`). `appVersion` null or missing omits the version suffix. `updatedAt` is formatted as a relative day and a timestamp. The server maps `current` by comparing a current-session id. This admin call does not pass one, so `current` is false for every row (`reference/immich/server/src/dtos/session.dto.ts:42-51`, `user-admin.service.ts:133-135`). A new User who has never signed in is an empty array. A thrown call fails the whole detail load.
- Source: `[id]/+layout.ts:22-26`, `[id]/+layout.svelte:206-213`, `DeviceCard.svelte:35-76`, `session.dto.ts:19-51`.

### `GET /api/admin/users/{id}/calendar-heatmap`

- When: after the detail page renders, not in the layout load (`[id]/+layout.svelte:221`).
- Method and path: `GET /api/admin/users/{id}/calendar-heatmap` with `from`, `to`, and `type`. `type` is `Upload` (`fetch-client.ts:4275-4290`, `CalendarHeatmapType` at `fetch-client.ts:8088-8090`). `from` and `to` are UTC dates. `to` is today. `from` is 51 weeks earlier, shifted back to Sunday (`reference/immich/web/src/lib/index.ts:23-28`). The client accepts only 200 (`user-admin.controller.ts:104-117`).
- Request: no body.
- Response fields the page requires: `from`, `series`, and `totalCount`. Each series item needs `date` and `count` (`reference/immich/web/src/lib/components/CalendarHeatmap.svelte:16-19`, `72-95`). A missing `series` throws on `.map`. A missing `totalCount` is drawn as an empty total. The template has no `{:catch}`, so a rejected call does not draw an empty grid (`[id]/+layout.svelte:221-229`). The rest of the detail page has already rendered. The server builds one series entry per day in the range, with `count` 0 when that day has no upload (`reference/immich/server/src/services/shared/user-methods.ts:24-35`). A User with no assets still gets that zero-filled series, not an empty `series`. A deleted User fails the lookup with 400 `User not found`, because this call uses `withDeleted: false` (`user-admin.service.ts:128-130`, `reference/immich/server/src/utils/misc.ts:113-119`). A just-created User is not deleted.
- Source: `[id]/+layout.svelte:216-229`, `index.ts:23-28`, `user-methods.ts:7-35`, `reference/immich/server/src/dtos/calendar-heatmap.dto.ts:22-34`.

### `GET /api/users/{id}/profile-image`

- When: the detail header, only when `profileImagePath` is non-empty (`UserAvatar.svelte:84-88`). A new User's default path is `''` (`1744910873969-InitialMigration.ts:111`), so this create path does not request it.
- Method and path: `GET /api/users/{id}/profile-image?updatedAt={profileChangedAt}` (`reference/immich/web/src/lib/utils.ts:175-187`, `254-256`, `reference/immich/packages/sdk/src/index.ts:21`, `90-91`). `updatedAt` is omitted when `profileChangedAt` is null or missing. This is an image element, not the JSON client. The handler sends the file (`user.controller.ts:246-255`). A missing user or an empty path is 404 (`reference/immich/server/src/services/user.service.ts:145-149`). A failed decode keeps the letter fallback (`UserAvatar.svelte:32-38`).
- Source: `UserAvatar.svelte:32-38`, `84-88`, `utils.ts:254-256`, `user.controller.ts:246-255`.

### `GET /api/public/config`

- When: the login page load, after root `init`, and only when the browser is not already signed in (`login/+page.ts:9-23`). Not the admin create path.
- Method and path: `GET /api/public/config`. The client accepts only 200 (`fetch-client.ts:6432-6438`).
- Request: no body.
- Response fields the page requires: `passwordLogin.enabled` shows the email and password form. Missing or false hides it, so a later User cannot submit (`login/+page.svelte:138`). `oauth.enabled` shows the OAuth button (`login/+page.svelte:156`). `server.loginPageMessage` non-empty draws the banner (`login/+page.svelte:131-135`). A missing message hides the banner. A thrown call fails the login load. The password form does not read the features body for this gate. It reads this public config.
- Source: `login/+page.ts:9-31`, `login/+page.svelte:131-156`.

### `POST /api/auth/login`

- When: the password form submits (`login/+page.svelte:80-84`).
- Method and path: `POST /api/auth/login`. Public. The client accepts only 201 (`fetch-client.ts:5175-5185`, `reference/immich/server/src/controllers/auth.controller.ts:26-50`). The handler sets no `@HttpCode`.
- Request: JSON `{ "email", "password" }` from the fields (`login/+page.svelte:84`, `auth.dto.ts:22-27`).
- Response fields the page requires: `isAdmin`, `shouldChangePassword`, and `isOnboarded`. The checks run in this order (`login/+page.svelte:86-104`). An admin with server config `isOnboarded` false goes to onboarding. A non-admin with `shouldChangePassword` true goes to `/auth/change-password` and returns, before `isOnboarded` is read. Otherwise a missing or false `isOnboarded` goes to onboarding. Otherwise the page opens `continue`, which defaults to `/photos` (`login/+page.ts:12`, `route.ts:95`, `158-166`). A missing `isAdmin` is falsy, so the admin branch is skipped. `!isAdmin` is then true, and `shouldChangePassword` true still opens the change-password screen. A missing `shouldChangePassword` is falsy, so that branch is skipped. A missing `isOnboarded` is falsy, so `!isOnboarded` opens onboarding. Any thrown call, including a status other than 201, stays on the form and shows the server message or `errors.incorrect_email_or_password` (`login/+page.svelte:106-109`). The handler sets the session cookies from `accessToken` (`auth.controller.ts:43-49`). The page does not read `accessToken`. The change-password screen does not use this JSON. It loads `GET /api/users/me` from the cookie.
- The login DTO's `isOnboarded` is the onboarding metadata, or false when that row is absent (`auth.dto.ts:42-56`). A later User created with no onboarding row is `isOnboarded: false`.
- Source: `login/+page.svelte:80-110`, `auth.controller.ts:30-50`, `auth.dto.ts:29-56`.

### `PUT /api/users/me` on the change-password screen

- When: the `/auth/change-password` form submits (`change-password/+page.svelte:22-28`). This is the screen a non-admin hits before the library. It is not `POST /api/auth/change-password`.
- Method and path: `PUT /api/users/me`. The client accepts only 200 (`fetch-client.ts:7628-7638`, `user.controller.ts:77-89`). The handler sets no `@HttpCode`.
- Request: JSON `{ "password": "<the new password>" }`. The form does not send the current password. The button enables when the two new passwords match and the confirm field is non-empty (`change-password/+page.svelte:19`). There is no 8-character check on this screen. The schema is `z.string().optional()` with no minimum (`user.dto.ts:14-18`).
- Response fields the page requires: none. Success navigates to `/auth/logout`, which calls `POST /api/auth/logout` and then the login page (`change-password/+page.svelte:27-28`, `reference/immich/web/src/routes/auth/logout/+page.svelte:5`, `auth-manager.svelte.ts:92-110`). The logout client accepts only 200 (`fetch-client.ts:5190-5197`, `auth.controller.ts:88-91`). A thrown logout still continues to login (`auth-manager.svelte.ts:100-102`). A thrown `PUT` has no catch on the form, so logout does not run and the library does not open. The server hashes a truthy `password` and sets `shouldChangePassword` to false (`user.service.ts:71-75`). A missing or empty `password` does not clear the flag. The next login then sees the flag clear and reads `isOnboarded`.
- Source: `change-password/+page.svelte:22-28`, `change-password/+page.ts:8-12`, `user.service.ts:56-79`, `user.dto.ts:11-24`.

### `POST /api/auth/change-password`

- When: the user-settings password form saves (`ChangePasswordSettings.svelte:12-14`, `reference/immich/web/src/lib/services/user.service.ts:50-59`). Not the screen before the library. Not the create path. The phone's use of this route is in [Login and server info](login-and-server-info.md).
- Method and path: `POST /api/auth/change-password`. The client accepts only 200. The handler sets `@HttpCode(HttpStatus.OK)` (`fetch-client.ts:5160-5170`, `auth.controller.ts:76-86`).
- Request: JSON `password` (the current password), `newPassword`, and `invalidateSessions` (`ChangePasswordSettings.svelte:14`). The button stays disabled until the current password and the new password are non-empty and the new password matches the confirm field (`ChangePasswordSettings.svelte:48`). The schema requires `newPassword` of at least 8 characters (`auth.dto.ts:70-76`). `invalidateSessions` omitted defaults to false.
- Response fields the page requires: none. Success clears the three fields and toasts. It does not navigate (`ChangePasswordSettings.svelte:15-18`, `user.service.ts:54-55`). A wrong current password is 400 `Wrong password` (`reference/immich/server/src/services/auth.service.ts:137-143`). Success stores the new hash, sets `shouldChangePassword` to false, and emits `AuthChangePassword` with the current session id and `invalidateSessions` (`auth.service.ts:145-158`).
- Source: `ChangePasswordSettings.svelte:12-48`, `user.service.ts:50-59`, `auth.controller.ts:76-86`, `auth.service.ts:137-158`, `auth.dto.ts:70-76`.

### `PUT /api/users/me/preferences` during onboarding

- When: the user-privacy step is destroyed, which is the next or previous click off that step (`OnboardingUserPrivacy.svelte:11-17`). A non-admin's steps are hello, theme, language, user privacy, and the mobile-app note (`onboarding/+page.svelte:45-96`, `112-118`). Theme writes a local preference only (`reference/immich/web/src/routes/auth/onboarding/OnboardingTheme.svelte:14`). Language writes the local language store and invalidates loads. It does not call the API (`reference/immich/web/src/lib/components/shared-components/settings/SettingsLanguageSelector.svelte:20-27`). Server steps (server privacy, storage template, backup) run only for an admin while server `isOnboarded` is false (`onboarding/+page.svelte:103-118`). A later non-admin does not run them, and does not call `POST /api/system-metadata/admin-onboarding`.
- Method and path: `PUT /api/users/me/preferences`. The client accepts only 200 (`fetch-client.ts:7743-7753`, `user.controller.ts:110-126`).
- Request: JSON `{ "cast": { "gCastEnabled": <the switch> } }`. The switch starts from `preferences.cast.gCastEnabled` (`OnboardingUserPrivacy.svelte:9`). The call runs even when the switch did not change.
- Response fields the page requires: the body is stored with `setPreferences` (`OnboardingUserPrivacy.svelte:14`). The step does not read a field off it before the next step. A thrown call toasts `errors.unable_to_update_settings` and does not stop the step change, because the request is in `onDestroy`. The server merges the patch onto the stored preferences (`user.service.ts:87-96`).
- Source: `OnboardingUserPrivacy.svelte:9-18`, `onboarding/+page.svelte:45-118`, `user.service.ts:87-96`.

### `PUT /api/users/me/onboarding`

- When: the last onboarding step's next click (`onboarding/+page.svelte:129-140`). For a non-admin, that is the mobile-app step. An admin also calls `POST /api/system-metadata/admin-onboarding` with `{ "isOnboarded": true }` first, then reloads server config (`onboarding/+page.svelte:131-134`, `fetch-client.ts:7317-7324`). A later non-admin skips that POST.
- Method and path: `PUT /api/users/me/onboarding`. The client accepts only 200 (`fetch-client.ts:7717-7727`, `user.controller.ts:183-192`).
- Request: JSON `{ "isOnboarded": true }` (`onboarding.dto.ts:4-6`).
- Response fields the page requires: none. Success navigates to `/photos` (`onboarding/+page.svelte:140`, `route.ts:95`). The page does not read `isOnboarded` off the response. A thrown call has no catch, so `/photos` does not open. The server stores that boolean and returns it (`user.service.ts:228-238`). A User with no onboarding row reads back as `{ "isOnboarded": false }` (`user.service.ts:208-216`).
- Source: `onboarding/+page.svelte:129-142`, `user.controller.ts:183-192`, `user.service.ts:208-238`, `onboarding.dto.ts:4-10`.
