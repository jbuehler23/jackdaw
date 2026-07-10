// ring.ts: fixed-capacity history buffer feeding the stats page sparklines.
export class RingBuffer {
  private buf: number[] = [];
  constructor(private capacity: number) {}

  push(value: number) {
    this.buf.push(value);
    if (this.buf.length > this.capacity) this.buf.shift();
  }

  values(): number[] {
    return this.buf;
  }
}
