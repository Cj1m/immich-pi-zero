# Immich on the Pi

A Rust server that speaks enough of the Immich API for the official mobile app and the official web UI to back up and browse photos and videos on a Raspberry Pi Zero 2 W.

## Language

**Contract**:
The HTTP calls the official Immich mobile app makes against server v3.3.6 on the success path: sign in with a password, upload photos and videos, view the timeline, show the memory lane and the memories list, open an asset, see thumbnails, play the original video, crop, rotate, or mirror an owned still and see its edited thumbnail and preview, use albums, leave likes and comments on a shared album, use trash and shared links, and show the faces, people, OCR, and search results the server has stored.
_Avoid_: Full API, OpenAPI parity, Immich-compatible, Web UI

**Web UI**:
The official Immich browser app shipped with server v3.3.6. A recipient opens a shared link in it.
_Avoid_: A new frontend, the mobile app, admin console

**ML service**:
The official Immich machine-learning process. The server sends it a local preview or a search string and stores the Asset results that come back. It does not hold the library.
_Avoid_: Ingest endpoint, file drop, on-device model

**User**:
An account on the server. The first User is an admin. An admin creates each later User, who is an admin only when created as one, and can later delete or restore them. A User sees their own assets, plus albums and shared links that include them, and a Partner's assets on the timeline when the recipient has chosen to show that library there.
_Avoid_: Account, member, owner

**Partner**:
A User who has shared their library with another User. The recipient chooses whether that library shows on their timeline.
_Avoid_: Collaborator, friend, shared user

**Asset**:
One User's photo or video. The same bytes uploaded by the same User are one Asset. The same bytes uploaded by another User are a different Asset.
_Avoid_: File, photo, media, item

**Edit**:
A crop, rotate, or mirror a User applies to an owned still. A still is an image that is not a motion photo and not animated. The viewer then shows the edited thumbnail and preview.
_Avoid_: Filter, adjustment, transcode

**Face**:
One person on an Asset, found by the server or marked by a User. The app shows that person's name, age, and thumbnail.
_Avoid_: Bounding box, overlay, tag

**OCR**:
One passage of text the server has read on an Asset, and the region where it sits. The app draws that region and the selected text, and search can find the Asset by the text.
_Avoid_: Face, caption, description

**CLIP vector**:
One vector the ML service has produced for an Asset. The vector for a search string lasts for that search.
_Avoid_: Embedding, feature, tag

**Original**:
The bytes of an Asset, kept in object storage for that User alone. The same bytes for that User are one Original.
_Avoid_: Master, source file, local copy

**Memory**:
One User's card of timeline assets for a calendar day, either that day in an earlier year or a person's birthday.
_Avoid_: Album, highlight, story

**Activity**:
A like or a comment a User leaves on a shared album, or on an Asset in that album.
_Avoid_: Reaction, notification, heatmap

**Session**:
A User signed in on one client. Logout ends that Session. Another sign-in is another Session.
_Avoid_: Token, cookie, access token
