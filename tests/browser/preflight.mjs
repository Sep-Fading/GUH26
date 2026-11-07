import { requireApplication } from '../support/harness.mjs';

export default async function preflight() {
  await requireApplication();
}
