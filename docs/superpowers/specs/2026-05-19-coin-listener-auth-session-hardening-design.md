# Coin Listener Auth Session Hardening Design

## 1. Goal

Replace the current demo login path with a focused production authentication and tenant-context baseline. Users should log in with hashed passwords, receive signed expiring tokens, and call protected API routes with an authenticated tenant context. The React app should persist the session intentionally, attach the token to API requests, and provide logout.

This milestone removes the highest-risk gap left after all-in-one packaging. It does not add full RBAC, user management screens, refresh tokens, password reset, OAuth, or multi-tenant administration.

## 2. Current context

The current backend login handler in `backend/crates/api-server/src/routes.rs` loads a user by email, compares `user.password_hash` directly to the plaintext request password, and returns `dev-token-{user.id}`. The API router does not protect `/api/*` routes with token validation. The seed migration in `backend/crates/storage/migrations/0002_config_management.sql` stores the default admin password as `admin`.

The frontend calls `login()` from `frontend/src/api/client.ts`, stores the returned `LoginResponse` only in React state in `frontend/src/App.tsx`, and never attaches an `Authorization` header. `frontend/src/pages/LoginPage.tsx` displays the default credentials and pre-fills the login form.

## 3. Approach options

### Option A: Minimal client-only persistence

Keep `dev-token-*`, persist it in the browser, and attach it to requests.

- Pros: smallest change.
- Cons: does not fix plaintext password comparison, forged tokens, or unprotected APIs.

### Option B: JWT bearer tokens with password hashing

Use Argon2id for password hashes, sign JWT bearer tokens with an API secret, validate tokens in Axum middleware, and attach tenant context to protected handlers. Store the frontend session in browser storage and send `Authorization: Bearer <token>` for API requests.

- Pros: fixes the immediate auth risks while keeping the deployment simple; works for both multi-process and all-in-one modes; creates the auth boundary needed before WebSocket work.
- Cons: logout is client-side until token expiry; no server-side revocation list.

### Option C: Server-side sessions with HttpOnly cookies

Create a sessions table, set HttpOnly cookies, validate each request through server-side session lookup, and add CSRF handling.

- Pros: stronger browser-token handling and server-side revocation.
- Cons: larger migration and API/frontend change; adds CSRF and cookie deployment concerns; unnecessary for the current focused milestone.

**Selected approach:** Option B. It closes the verified security gaps without expanding into account administration or a full session service. Option C remains a later hardening step if server-side revocation or cookie-only auth becomes required.

## 4. Backend authentication model

Add an auth module under `backend/crates/api-server/src/auth.rs` with three responsibilities:

1. Password verification with Argon2id using the existing `users.password_hash` field.
2. Token issuing and validation for signed JWT bearer tokens.
3. Auth context extraction for protected routes.

Add backend config values:

- `AUTH_TOKEN_SECRET`: required for `api-server` and `all-in-one` startup. Empty values are invalid.
- `AUTH_TOKEN_TTL_SECONDS`: token lifetime, default `43200` seconds.

The shared `AppConfig` can carry an `AuthConfig`, but only API startup paths must fail when the secret is missing. Scheduler, worker, and notifier should not require auth config because they do not serve user traffic.

JWT claims should include:

- `sub`: user UUID as a string.
- `tenant_id`: active tenant UUID as a string.
- `email`: user email.
- `iat`: issued-at Unix timestamp.
- `exp`: expiry Unix timestamp.

The login response keeps the current shape (`token`, `user`, `tenant`) so frontend and API clients do not need a schema-breaking response change.

## 5. Password hashing and seed migration

Use Argon2id through the Rust `argon2` crate. The login handler should verify the submitted password against `users.password_hash`; it must not accept plaintext equality as a fallback.

Add a new migration, `backend/crates/storage/migrations/0012_auth_session_baseline.sql`, that updates the seeded admin user only when it still has the legacy plaintext hash:

- Match `email = 'admin@example.com'` and `password_hash = 'admin'`.
- Replace the value with a precomputed Argon2id hash for the same local bootstrap password.
- Leave all other users unchanged.

Do not rewrite existing deployed user hashes blindly. Future user creation can use the same hashing helper, but user-management flows are outside this milestone.

`frontend/src/pages/LoginPage.tsx` should stop displaying or pre-filling the default password. Keeping `admin@example.com` as a convenience email placeholder is acceptable; showing `admin / admin` is not.

## 6. Protected API routing and tenant context

`/health` and `POST /api/auth/login` remain public. All other `/api/*` routes require a valid bearer token.

The middleware should:

1. Read `Authorization: Bearer <token>`.
2. Validate the token signature and expiry.
3. Parse `user_id` and `tenant_id` from claims.
4. Confirm the user is active and still belongs to the tenant.
5. Insert an `AuthContext` into request extensions.

Unauthenticated or invalid requests return `401`. Authenticated users without tenant membership return `403`. Error bodies should continue using the existing `{ "error": "..." }` shape.

Handlers that work with tenant-scoped data should use the authenticated tenant from `AuthContext`, not a client-supplied tenant value. For create-address requests, the server should ignore or overwrite `tenant_id` with the authenticated tenant. Existing global configuration reads such as chains, assets, and providers still require authentication but do not need tenant filtering in this milestone because the current data model treats them as global chain/provider configuration.

## 7. Frontend session behavior

Add a small frontend session boundary, for example `frontend/src/auth/session.ts`, responsible for:

- Saving a successful `LoginResponse` to browser storage.
- Loading a stored session on app startup.
- Clearing the session on logout or unauthorized API responses.
- Returning the current token to the API client.

Use one storage key such as `coin-listener.session.v1`. `localStorage` is acceptable for this milestone because the current app has no cookie/session endpoint; the implementation should keep token access centralized so a future HttpOnly-cookie migration is isolated.

Update `frontend/src/api/client.ts` so every API request except login includes `Authorization: Bearer <token>` when a session exists. If an API call returns `401`, clear the stored session and surface the error so the app can return to the login page.

Update `frontend/src/App.tsx` to initialize session state from storage, write the session after login, and expose a logout action in the app shell. The logout action should clear storage and reset the in-memory session.

## 8. Data flow

Login flow:

1. User submits email and password from the login page.
2. Backend loads the user by email.
3. Backend rejects missing, inactive, or password-mismatched users with `401`.
4. Backend loads the user's default tenant and returns `403` when the user has no active tenant membership.
5. Backend signs a JWT containing user and tenant claims.
6. Frontend stores the returned session and renders the authenticated app shell.

Authenticated API flow:

1. Frontend request helper reads the stored token.
2. Request helper sends `Authorization: Bearer <token>`.
3. Backend middleware validates token and membership.
4. Handler reads `AuthContext` for tenant-scoped operations.
5. `401` clears the frontend session and returns the user to login.

## 9. Error handling

Backend behavior:

- Missing or malformed bearer token: `401`.
- Expired token: `401`.
- Invalid signature: `401`.
- Inactive user: `401`.
- Missing or inactive tenant membership: `403`.
- Missing `AUTH_TOKEN_SECRET` in API startup: configuration error before serving requests.

Frontend behavior:

- Login failures show the existing Semi `Toast.error` path.
- `401` from any authenticated request clears session storage and shows the login page.
- Logout is always local and does not require a backend call.

## 10. Testing strategy

Use TDD for behavior changes.

Required backend tests:

- Argon2 password verification accepts the seeded hash and rejects wrong passwords.
- JWT issuing and validation round-trips user and tenant claims.
- Expired or tampered JWTs are rejected.
- Protected API routes reject missing tokens.
- Public routes `/health` and `/api/auth/login` remain reachable without a token.
- Tenant-scoped address creation uses the authenticated tenant instead of a request-provided tenant.
- The new migration updates only the legacy seeded admin plaintext hash.

Required frontend checks:

- API client attaches `Authorization` when a stored session exists.
- API client does not attach auth headers to login.
- `401` clears stored session.
- App initializes from stored session and logout clears it.
- Login page no longer displays the default password.

Required verification commands:

- `cargo fmt --all --check --manifest-path backend/Cargo.toml`
- `cargo test --workspace --manifest-path backend/Cargo.toml`
- `npm run build --prefix frontend`

## 11. Acceptance criteria

This milestone is complete when:

1. Backend login verifies Argon2 password hashes and no longer compares plaintext passwords.
2. Backend login returns signed expiring bearer tokens instead of `dev-token-*` values.
3. All `/api/*` routes except login require bearer auth.
4. Protected handlers can access authenticated user and tenant context.
5. Tenant-scoped writes cannot be assigned to a tenant supplied by the client.
6. The seeded admin password is stored as an Argon2 hash after migrations.
7. Frontend persists the session, attaches the token to API calls, clears it on `401`, and supports logout.
8. Login UI no longer displays the default password.
9. Backend tests and frontend build verification pass.

## 12. Non-goals

- Role-based permissions beyond confirming tenant membership.
- User invitation, creation, password reset, or profile management UI.
- Refresh tokens or server-side token revocation.
- HttpOnly cookie sessions and CSRF handling.
- WebSocket authentication.
- Deployment runbooks or secret-management automation beyond documenting `AUTH_TOKEN_SECRET` in `.env.example`.
