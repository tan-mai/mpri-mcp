"""Market Pricing MCP server."""

import os

__version__ = "0.1.0"
# Commit the Docker image was built from (GIT_SHA build arg, set by CI); "dev" from source.
__commit__ = os.environ.get("MPRI_COMMIT", "dev")[:7]
