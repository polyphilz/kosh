type SearchCheckpoint = () => Promise<void>;

let activeCheckpoint: SearchCheckpoint | null = null;

export function registerSearchCheckpoint(checkpoint: SearchCheckpoint): () => void {
  activeCheckpoint = checkpoint;
  return () => {
    if (activeCheckpoint === checkpoint) activeCheckpoint = null;
  };
}

export async function checkpointBeforeSearch(): Promise<void> {
  await activeCheckpoint?.();
}
