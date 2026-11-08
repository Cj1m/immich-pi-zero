# Decide which cluster-group responses this server matches

Type: grilling
Status: resolved

## Question

Sharing settings calls the cluster-group routes: list requests, create a request, accept, leave, list users, and regenerate people. [Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md) says creating a User does not add a cluster, and `clusterGroupId` may be null. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those cluster-group calls match v3.3.6?

## Comments

Recommendation accepted: the cluster-group calls match v3.3.6. Creating a User still does not add a cluster, and `clusterGroupId` may stay null, as that ticket set. These routes do not create the first group. There is no Pi difference in the calls themselves.

## Answer

`cluster_group` keeps `id`, `name` (null by default), `createdAt`, `updatedAt`, and `updateId`. `cluster_group_request` keeps `id`, `clusterGroupId`, `userId`, and `createdAt`. The pair `clusterGroupId` and `userId` is unique. A request document is `{id, clusterGroupId, userId, createdAt}`.

`GET /api/cluster-groups/requests` returns 200 of requests whose `userId` is the caller, oldest first. None is `[]`. `GET /api/cluster-groups/{id}/requests` returns 200 of that group's requests, oldest first. The caller must be a member. No access is 400 `Not found or no clusterGroup.read access`. `GET /api/cluster-groups/{id}/users` returns 200 of the public user document for members who are not deleted. The caller is first, then name ascending. The page reads `id` and `name`. The same access miss is 400 `Not found or no clusterGroup.read access`. An invited user can read the group and its users. An invited user cannot read the group's outgoing requests.

`PUT /api/cluster-groups/{id}/requests` body is `{userId}`. A new request returns 201 of the document. The same pair again returns 200 of the existing document and does not send another notification. Asking yourself is 400 `Cannot request to join your own cluster group`. An unknown user is 400 `User not found`. The caller must be a member. No access is 400 `Not found or no clusterGroupRequest.create access`. A new request writes a notification for that user: type `ClusterGroupRequest`, level `info`, title `Cluster Group Request`, description `{senderName} asked you to join their cluster group`, and `data` the JSON string `{"clusterGroupId":"..."}`. The bell refreshes from `on_notification`. No email is sent.

`POST /api/cluster-groups/requests/{id}/accept` returns 204. The caller's `clusterGroupId` becomes the request's group, and the request row is removed. A person group that only this user owns moves to that cluster. A person group someone else also owns stays, and this user's people in it move to a new person group. The next person sync shows those ids. No access is 400 `Not found or no clusterGroupRequest.read access`. A missing request is 400 `Request not found`.

`DELETE /api/cluster-groups/requests/{id}` returns 204 and removes the row. The caller may be the invited user or a member of the group. No access is 400 `Not found or no clusterGroupRequest.delete access`.

`POST /api/cluster-groups/{id}/leave` returns 204. The caller must be a member, and another member who is not deleted must remain, or the call is 400 `Cannot leave a cluster group without any other members`. Leave creates a new `cluster_group` with `name` null and moves the caller onto it, with the same person-group split as accept. No access is 400 `Not found or no clusterGroup.leave access`.

`POST /api/cluster-groups/{id}/regenerate-people` returns 204 and queues the existing face job for that cluster with `force` true. The Pi still does not run the model. No access is 400 `Not found or no clusterGroup.read access`.

A path id that is not a uuid is 400. Until `clusterGroupId` is set, sharing settings and the people page call these routes with a null id, that call fails, and each page shows its load error. The people list itself still loads. Nothing on this screen creates the first group. Accept and leave only move a user who already has a group, or who was invited into one.

Unsigned is 401. A shared link is 403 `Forbidden`. The next user sync sends the new `clusterGroupId`.
