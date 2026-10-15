import { ApiError as RuntimeApiError } from "../../../worker/errors.js";

export interface ErrorDetail {
  code: string;
  message: string;
  field?: string;
}

/** The shared facade error, with sanitized Apple details and HTTP-style status. */
export interface ApiError extends Error {
  status: number;
  code: string;
  retryable: boolean;
  details: ErrorDetail[];
  headers: Record<string, string>;
}

// Keep instanceof checks identical to the Worker without introducing another error class.
export const ApiError = RuntimeApiError as new (
  status: number,
  code: string,
  message: string,
  options?: { retryable?: boolean; details?: ErrorDetail[]; headers?: Record<string, string> },
) => ApiError;
