// Yulius: the package's telemetry (a POST of the page's domain, browser and frame timings to
// shaders.com for a share of sessions) is replaced by this no-op, so the vendored code never
// contacts a third party. Same exports as the original module.
class TelemetryCollector {
  constructor() {}
  start() {}
  stop() {}
  destroy() {}
}
function isExternalUser() { return false; }
function shouldCollectTelemetry() { return false; }
function startTelemetry() { return null; }
export { TelemetryCollector, isExternalUser, shouldCollectTelemetry, startTelemetry };
