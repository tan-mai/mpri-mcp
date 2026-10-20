"""Errors raised by the services layer. MCP tools turn them into ToolError, the dashboard into
flash messages, so messages must be safe and useful to show to a person or a model."""


class ServiceError(Exception):
    pass


class NotFoundError(ServiceError):
    pass


class ValidationError(ServiceError):
    pass


class ConflictError(ServiceError):
    pass
