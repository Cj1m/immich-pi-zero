# Decide how the first User is created

Type: grilling
Status: resolved

## Question

The official app never calls `POST /api/auth/admin-sign-up` and does not read `isInitialized`. A server with no User shows the login form and cannot create the admin from the phone. The first User is the admin. How is that User created on this server?

## Comments

Recommendation accepted: the admin's email, name, and password live in the same config as the object-storage credentials. Startup creates the admin only when the server has no User, and stores the password hash.

Recommendation accepted: a first boot with any of those three missing refuses to start and names the missing value. Once a User exists, later boots ignore the three config values.

## Answer

The first User is created from server config, before anyone signs in. The admin's email, name, and password sit in the same config as the object-storage credentials (environment variables or the one config file).

On startup, when the server has no User, it creates that User as the admin and stores only the password hash. If the email, the name, or the password is missing, the process refuses to start and names the missing value. The phone then signs in with `POST /api/auth/login`.

Once a User exists, startup ignores those three config values. Absence and later edits do not change the stored hash, and login checks that hash. `POST /api/auth/admin-sign-up` is not part of this server. The app has no first-user screen, and it never creates a User.
