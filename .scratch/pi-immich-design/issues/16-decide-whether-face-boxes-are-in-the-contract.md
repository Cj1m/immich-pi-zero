# Decide whether face boxes are in the Contract

Type: grilling
Status: resolved

## Question

[Inventory how the app reads ML results](06-inventory-how-the-app-reads-ml-results.md) finds that a face row is synced with a bounding box, and that no screen draws the box. What the user sees is the person's name, age, and `GET /api/people/{id}/thumbnail`. The Contract says the app shows the faces the server stored. Does that stop at the people row, with drawing the box outside the Contract?

## Comments

Recommendation accepted: a Face is one person the server has found on an Asset. The app shows that person's name, age, and thumbnail. Drawing a box is outside the Contract. The box, the image size, and the source type still travel on `AssetFaceV3` so the row can land.

## Answer

A Face is in the Contract. It is one person the server has found on an Asset. The people list, the person page, and the asset details sheet show that person's name, age, and thumbnail. They find the person through the face: who it belongs to, whether it is visible, and whether it was deleted. A face with no person does not appear, because both screens join through the person.

Drawing a box on the photo is outside the Contract. No screen draws it.

The box, the image size, and the source type still travel on `AssetFaceV3`. Those fields are required, and a missing one throws before the row is stored. The app writes them. No screen reads them. Sending them is how the face arrives.

The thumbnail stays `GET /api/people/{id}/thumbnail`. Where those bytes live is [Inventory where the person thumbnail lives](18-inventory-where-the-person-thumbnail-lives.md). None of the eight constraints in [Decide which Pi constraints the app can see](08-decide-which-pi-constraints-the-app-can-see.md) remove this. The face row is metadata on the Pi.
