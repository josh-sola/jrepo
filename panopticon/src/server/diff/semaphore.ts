// Hand-rolled so the batch stays a plain `Promise.all` with no dependency.
export class Semaphore {
  private readonly max: number;
  private active = 0;
  private readonly queue: (() => void)[] = [];

  constructor(max: number) {
    this.max = max;
  }

  async run<T>(task: () => Promise<T>): Promise<T> {
    await this.acquire();
    try {
      return await task();
    } finally {
      this.release();
    }
  }

  private acquire(): Promise<void> {
    if (this.active < this.max) {
      this.active += 1;
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      this.queue.push(() => {
        this.active += 1;
        resolve();
      });
    });
  }

  private release(): void {
    this.active -= 1;
    const next = this.queue.shift();
    if (next !== undefined) next();
  }
}
