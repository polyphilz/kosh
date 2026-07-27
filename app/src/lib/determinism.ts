export interface Clock {
  nowMs(): number;
}

export interface IdGenerator {
  nextId(kind: string): string;
}

export class FixedClock implements Clock {
  private readonly value: number;

  constructor(value: number) {
    this.value = value;
  }

  nowMs(): number {
    return this.value;
  }
}

export class SequenceIdGenerator implements IdGenerator {
  private next = 0;

  nextId(kind: string): string {
    this.next += 1;
    return `${kind}-${this.next}`;
  }
}

export const systemClock: Clock = {
  nowMs: () => Date.now(),
};

export const cryptoIdGenerator: IdGenerator = {
  nextId: (kind) => `${kind}-${crypto.randomUUID()}`,
};
