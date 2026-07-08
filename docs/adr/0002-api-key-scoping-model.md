# OAuth2 session users get full access; API keys are scoped

OAuth2 browser-authenticated users (session cookies) receive full access to all endpoints. API keys are scoped to `inference`, `management:read`, or `management:write`. This keeps the web UI simple (logged-in humans get everything) while providing granular control for machine-to-machine access.

**Considered Options:** Scope both OAuth2 users and API keys uniformly (rejected — adds complexity to the web UI flow where humans need full management access), or any-authed-user-full-access (rejected — loses the ability to restrict API key permissions).

**Consequences:** If future work requires scoping OAuth2 users (e.g., multi-tenant), the `AuthSubject::User` type will need a scopes field and the scope middleware will need to check it. The current design makes this additive rather than requiring a rewrite.
