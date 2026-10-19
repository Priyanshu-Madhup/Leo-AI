/**
 * Decides whether sound picked up while Leo is talking is really the user.
 *
 * Any sound is not enough. The microphone can carry Leo's own voice (what the
 * echo canceller leaves behind), room noise or someone else. So an interruption
 * needs several things at once:
 *  - the detector is confident it is speech (a higher bar than when Leo is quiet),
 *  - it lasts long enough (a cough or a click is shorter),
 *  - and while Leo's voice is loud, the bar goes up further, because that is
 *    exactly when leftover echo is most likely.
 *
 * Pure logic with no audio inside, so it can be tested directly. All numbers
 * are starting points to tune on real hardware.
 */
export interface BargeInConfig {
  /** Speech probability needed while Leo is talking. */
  speakingThreshold: number;
  /** Extra probability required while Leo's output is loud. */
  loudOutputBonus: number;
  /** Output level (0..1) above which Leo counts as loud. */
  loudOutputLevel: number;
  /** How long speech must last before it counts as an interruption. */
  minSpeechMs: number;
  /** Frame length of the detector in ms. */
  frameMs: number;
  /** Single dips below the bar that do not restart the count. */
  allowedDips: number;
}

export const DEFAULT_BARGE_IN: BargeInConfig = {
  speakingThreshold: 0.7,
  loudOutputBonus: 0.12,
  loudOutputLevel: 0.25,
  minSpeechMs: 260,
  frameMs: 32,
  allowedDips: 2,
};

export class BargeInDetector {
  private run = 0;
  private dips = 0;

  constructor(private config: BargeInConfig = DEFAULT_BARGE_IN) {}

  /** Feed one detector frame. Returns true once, when an interruption is confirmed. */
  push(probability: number, outputLevel: number): boolean {
    const c = this.config;
    const bar = c.speakingThreshold + (outputLevel > c.loudOutputLevel ? c.loudOutputBonus : 0);
    if (probability >= bar) {
      this.run++;
      this.dips = 0;
    } else if (this.run > 0 && this.dips < c.allowedDips) {
      this.dips++;
    } else {
      this.reset();
    }
    if (this.run * c.frameMs >= c.minSpeechMs) {
      this.reset();
      return true;
    }
    return false;
  }

  reset(): void {
    this.run = 0;
    this.dips = 0;
  }
}
