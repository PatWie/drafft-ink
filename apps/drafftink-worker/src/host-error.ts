/**
 * A failure of the room's storage, thrown to the Rust core. The core reads `kind` to tell a used-up daily limit
 * (which maps to `ErrorCode::DailyLimit`) from any other failure (`Unavailable`).
 */
export class HostFailure extends Error {
  constructor(
    readonly kind: "daily_limit" | "storage",
    message: string,
  ) {
    super(message);
    this.name = "HostFailure";
  }
}

// The error texts of the free plan's daily limits are not documented (docs/design/collab-spike.md, section 7).
// This is the best guess; the first real-account run must replace it with the exact text.
const QUOTA_TEXT = /(daily|free tier|free plan|quota|spending cap|exceeded (the )?(account|allowed|limit))/i;

/** Whether an exception from the platform looks like a used-up daily limit or spending cap. */
export function isQuotaError(error: unknown): boolean {
  if (error instanceof HostFailure) return error.kind === "daily_limit";
  return error instanceof Error && QUOTA_TEXT.test(error.message);
}

/** The typed failure for any exception of a storage call. */
export function failureOf(error: unknown): HostFailure {
  if (error instanceof HostFailure) return error;
  const message = error instanceof Error ? error.message : "the storage failed";
  return new HostFailure(isQuotaError(error) ? "daily_limit" : "storage", message);
}

/** Runs a storage call and turns any exception into a [`HostFailure`]. */
export function guard<T>(call: () => T): T {
  try {
    return call();
  } catch (error) {
    throw failureOf(error);
  }
}
