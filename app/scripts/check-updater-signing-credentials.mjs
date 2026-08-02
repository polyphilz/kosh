import { takeUpdaterEnvironment, verifyUpdaterSigningCredentials } from "./updater-signing.mjs";

verifyUpdaterSigningCredentials(takeUpdaterEnvironment());
console.info("Updater signing credentials passed.");
