const unavailable = (): never => {
  throw new Error("drizzle-orm/bun-sqlite is unavailable under node");
};

export const drizzle = unavailable;
export const migrate = unavailable;
