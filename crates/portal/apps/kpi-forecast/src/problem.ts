/** A problem+json answer of the App's server as an error whose message is its `detail`. */
export class ServerProblem extends Error {
  constructor(
    readonly status: number,
    detail: string,
  ) {
    super(detail);
  }
}
