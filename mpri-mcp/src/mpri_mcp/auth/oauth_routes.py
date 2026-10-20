"""Authorization-server routes: the MCP SDK's handlers, with our own metadata document."""

from __future__ import annotations

from mcp.server.auth.handlers.metadata import MetadataHandler
from mcp.server.auth.routes import build_metadata, cors_middleware, create_auth_routes
from mcp.server.auth.settings import ClientRegistrationOptions, RevocationOptions
from pydantic import AnyHttpUrl
from starlette.routing import Route

from mpri_mcp.auth.oauth_provider import OFFLINE_ACCESS, SqliteOAuthProvider
from mpri_mcp.config import Settings

METADATA_PATH = "/.well-known/oauth-authorization-server"


def build_oauth_routes(settings: Settings, provider: SqliteOAuthProvider) -> list[Route]:
    issuer = AnyHttpUrl(settings.issuer_url)
    registration = ClientRegistrationOptions(
        enabled=True,
        valid_scopes=[settings.oauth_scope, OFFLINE_ACCESS],
        default_scopes=[settings.oauth_scope],
    )
    revocation = RevocationOptions(enabled=True)
    routes = [
        r
        for r in create_auth_routes(
            provider=provider,  # type: ignore[arg-type]
            issuer_url=issuer,
            client_registration_options=registration,
            revocation_options=revocation,
        )
        if r.path != METADATA_PATH
    ]

    metadata = build_metadata(issuer, None, registration, revocation)
    # Public clients (DCR with token_endpoint_auth_method=none) are supported by the SDK's
    # authenticator; say so. And we put `iss` on authorization responses (RFC 9207).
    auth_methods = ["client_secret_post", "client_secret_basic", "none"]
    metadata.token_endpoint_auth_methods_supported = auth_methods
    metadata.revocation_endpoint_auth_methods_supported = auth_methods
    metadata.authorization_response_iss_parameter_supported = True
    routes.insert(
        0,
        Route(
            METADATA_PATH,
            endpoint=cors_middleware(MetadataHandler(metadata).handle, ["GET", "OPTIONS"]),
            methods=["GET", "OPTIONS"],
        ),
    )
    return routes
