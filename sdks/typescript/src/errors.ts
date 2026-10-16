export interface ErrorDetail {
  code: string;
  message: string;
  field?: string;
}

/** A sanitized facade error returned by the shared Rust core. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly retryable: boolean;
  readonly details: ErrorDetail[];
  readonly headers: Record<string, string>;
  readonly retryAfter?: number;

  constructor(status: number, code: string, message: string,
    options: { retryable?: boolean; details?: ErrorDetail[]; headers?: Record<string, string>; retryAfter?: number | null } = {}) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.retryable = options.retryable ?? false;
    this.details = options.details ?? [];
    this.retryAfter = options.retryAfter ?? undefined;
    this.headers = options.headers ?? (options.retryAfter != null ? { "Retry-After": String(options.retryAfter) } : {});
  }

  static fromCore(error: { status: number; code: string; message: string; retryable: boolean;
    details: ErrorDetail[]; retryAfter?: number | null }): ApiError {
    return new ApiError(error.status, error.code, error.message, error);
  }
}
