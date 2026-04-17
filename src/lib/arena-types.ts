export type ArenaContestant = {
  id: string;
  sessionId: string;
  model: string;
  label: string;
  status: string;
  response: string | null;
  createdAt: number;
  updatedAt: number;
};

export type ArenaRun = {
  id: string;
  prompt: string;
  cwd: string;
  status: "running" | "completed";
  createdAt: number;
  updatedAt: number;
  contestants: ArenaContestant[];
};

export type ArenaListPayload = {
  runs: ArenaRun[];
};
