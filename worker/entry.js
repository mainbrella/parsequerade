import { DurableObject } from "cloudflare:workers";
import worker from "./index.js";
import { HostedConnection as ConnectionStorage } from "./connections.js";

export class HostedConnection extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.connection = new ConnectionStorage(ctx, env);
  }

  fetch(request) { return this.connection.fetch(request); }
}

export default worker;
