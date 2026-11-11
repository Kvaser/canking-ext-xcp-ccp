// Shared "print an error to stderr and exit 1" helper for the ck-cli scripts.
export function fail(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

// Importing this also reports any uncaught exception that way.
process.on("uncaughtException", (e) => fail(e?.message ?? e));
