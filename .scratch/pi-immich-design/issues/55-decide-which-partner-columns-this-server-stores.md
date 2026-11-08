# Decide which partner columns this server stores

Type: grilling
Status: resolved

## Question

[Decide which partner responses this server matches](47-decide-which-partner-responses-this-server-matches.md) writes the partner row and syncs `PartnerV1` and `PartnerDeleteV1`. v3.3.6 stores that row in Postgres. This server stores it in SQLite. Which partner columns does this server keep?

## Comments

Recommendation accepted: keep the v3.3.6 partner columns. `inTimeline` defaults to false.

## Answer

`partner` keeps `sharedById`, `sharedWithId`, `createdAt`, `createId`, `updatedAt`, `inTimeline`, and `updateId`. `sharedById` and `sharedWithId` are the primary key. `inTimeline` is boolean and defaults to false. `createId` is what the recipient's asset backfill follows. `updateId` is the `PartnerV1` ack.

A deleted row writes `partner_audit` with `id`, `sharedById`, `sharedWithId`, and `deletedAt`. That is `PartnerDeleteV1`. The ack cursor is the audit `id`.
