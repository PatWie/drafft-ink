/**
 * Turns the core's `wakeAt` into a timer or an alarm.
 *
 * - A wake within a minute (flush, presence tick, compaction advice, a join that did not come, the grace after the
 *   last person left) is a `setTimeout`. A pending timer keeps the object awake, which is what a room with people
 *   needs; it stops by itself when the core has nothing near to do.
 * - A farther wake (the retention deadline, 48 hours after the last visit) is an alarm only. A pending alarm does
 *   not keep the object awake, so an empty room sleeps for free and the platform wakes it at the deadline.
 *
 * `setAlarm` and `getAlarm` are storage calls that count as rows, so the alarm is written only when it differs from
 * the one that is set, and what is set is remembered for the life of the instance.
 */

/** Wakes at most this far ahead are timers. */
export const NEAR_MS = 60_000;
/** A timer never fires sooner than this after it was set, so a wake in the past cannot spin. */
const MIN_DELAY_MS = 20;

export class WakeScheduler {
  #timer: ReturnType<typeof setTimeout> | undefined;
  /** The alarm as far as this instance knows: `undefined` when it has not been read yet. */
  #armed: number | null | undefined;

  constructor(
    private readonly storage: DurableObjectStorage,
    private readonly now: () => number,
    private readonly onTimer: () => void,
  ) {}

  /** Follows the core's latest `wakeAt`. `undefined`: the core needs no wake, and an alarm that is set stays. */
  async follow(wakeAt: number | undefined): Promise<void> {
    this.#clearTimer();
    if (wakeAt === undefined) return;
    const delay = wakeAt - this.now();
    if (delay <= NEAR_MS) {
      this.#timer = setTimeout(() => {
        this.#timer = undefined;
        this.onTimer();
      }, Math.max(delay, MIN_DELAY_MS));
      return;
    }
    if (this.#armed === undefined) this.#armed = await this.storage.getAlarm();
    if (this.#armed !== wakeAt) {
      await this.storage.setAlarm(wakeAt);
      this.#armed = wakeAt;
    }
  }

  /** The alarm fired: it is consumed. */
  alarmFired(): void {
    this.#armed = null;
  }

  /** Stops the timer. The alarm stays. */
  stop(): void {
    this.#clearTimer();
  }

  /** Forgets everything, for a room that was deleted (its storage, alarm included, is wiped by the caller). */
  reset(): void {
    this.#clearTimer();
    this.#armed = null;
  }

  /** Sets an alarm after `delayMs`, to try again after a failure. Best effort. */
  async retryLater(delayMs: number): Promise<void> {
    const at = this.now() + delayMs;
    await this.storage.setAlarm(at);
    this.#armed = at;
  }

  #clearTimer(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
  }
}
