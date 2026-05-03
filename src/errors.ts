export class NotImplementedError extends Error {
  constructor(feature: string) {
    super(`${feature} is not implemented yet. Run npm run test:compat to drive the TypeScript port against Python pdfplumber goldens.`);
    this.name = "NotImplementedError";
  }
}

export function namedError(name: string, message: string): Error {
  const error = new Error(message);
  error.name = name;
  return error;
}
