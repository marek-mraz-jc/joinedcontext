/** A problem+json answer as an error whose message is its `detail`. */
export class ServerProblem extends Error {
  constructor(
    readonly status: number,
    detail: string,
  ) {
    super(detail);
  }
}
