"""Dashboard pages."""

from __future__ import annotations

from fastapi import APIRouter, Depends, Request
from starlette.responses import Response

from mpri_mcp.admin.web import render, require_admin
from mpri_mcp.db.models import AdminUser

router = APIRouter()


@router.get("/admin")
async def overview(request: Request, admin: AdminUser = Depends(require_admin)) -> Response:
    return render(request, "overview.html")
