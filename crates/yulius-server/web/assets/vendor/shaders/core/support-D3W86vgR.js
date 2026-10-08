var GpuUnavailableError = class extends Error {
	reason;
	constructor(reason, message, options) {
		super(message ?? `[gpu] WebGPU unavailable (${reason})`);
		this.name = "GpuUnavailableError";
		this.reason = reason;
		if (options?.cause !== void 0) this.cause = options.cause;
	}
};
function isGpuUnavailableError(error) {
	return !!error && typeof error === "object" && error.name === "GpuUnavailableError" && typeof error.reason === "string";
}
var debugOverride = null;
function setShadersDebug(enabled) {
	debugOverride = enabled;
}
function isShadersDebug() {
	if (debugOverride !== null) return debugOverride;
	const flag = globalThis.__SHADERS_DEBUG__;
	if (flag !== void 0 && flag !== null) return !!flag;
	try {
		if (typeof localStorage !== "undefined" && localStorage.getItem("shaders:debug") === "1") return true;
	} catch {}
	return false;
}
var customWgslUsed = false;
function markCustomWgslUsed() {
	customWgslUsed = true;
}
function hasCustomWgsl() {
	return customWgslUsed;
}
function authorError(...args) {
	if (isShadersDebug() || customWgslUsed) console.error(...args);
}
function debugWarn(...args) {
	if (isShadersDebug()) console.warn(...args);
}
function debugError(...args) {
	if (isShadersDebug()) console.error(...args);
}
function isWebGPUSupported() {
	return typeof navigator !== "undefined" && !!navigator.gpu;
}
var probeCache = null;
function getWebGPUSupport() {
	if (probeCache) return probeCache;
	probeCache = (async () => {
		if (!isWebGPUSupported()) return {
			supported: false,
			reason: "unsupported"
		};
		try {
			if (!await navigator.gpu.requestAdapter({ powerPreference: "high-performance" })) return {
				supported: false,
				reason: "no-adapter"
			};
			return { supported: true };
		} catch (error) {
			debugError("[gpu] adapter probe threw:", error);
			return {
				supported: false,
				reason: "no-adapter"
			};
		}
	})();
	return probeCache;
}
var PAGE_WIDE_REASONS = new Set([
	"unsupported",
	"no-adapter",
	"no-device",
	"out-of-memory",
	"gpu-error"
]);
var gpuUnusableReason = null;
function markGpuUnusable(reason) {
	if (gpuUnusableReason || !PAGE_WIDE_REASONS.has(reason)) return;
	gpuUnusableReason = reason;
}
function getGpuUnusableReason() {
	return gpuUnusableReason;
}
export { getGpuUnusableReason as a, isGpuUnavailableError as c, markCustomWgslUsed as d, markGpuUnusable as f, debugWarn as i, isShadersDebug as l, authorError as n, getWebGPUSupport as o, setShadersDebug as p, debugError as r, hasCustomWgsl as s, GpuUnavailableError as t, isWebGPUSupported as u };
