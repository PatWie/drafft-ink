/**
 * What a Worker isolate remembers once a Durable Object call failed because the daily limit is used up: until
 * 00:00 UTC every request that would need an object is answered at the edge, so a spent quota costs no more
 * object requests. The state is per isolate; another isolate learns it from its own first failed call.
 */
export class QuotaGate {
  #until = 0;

  /** Notes a failed call at `now`; `resetAt` is the next 00:00 UTC. */
  trip(resetAt: number): void {
    this.#until = Math.max(this.#until, resetAt);
  }

  /** Seconds until the limit resets when it is used up at `now`, else `null`. */
  retryAfterSeconds(now: number): number | null {
    return now < this.#until ? Math.ceil((this.#until - now) / 1000) : null;
  }
}
