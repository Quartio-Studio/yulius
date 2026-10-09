import { createGpuUniformsMap, debugError, getRegisteredShader, resolveBoundingBox, rootPassthrough, shaderRendererGPU } from "../core/index.js";
import { getAllShaders } from "../core/registry.js";
import { isExternalUser, startTelemetry } from "../core/telemetry/index.js";
function isPropDriver(value) {
	if (typeof value !== "object" || value === null || !("type" in value)) return false;
	const maybe = value;
	return maybe.type === "map" || maybe.type === "mouse" || maybe.type === "mouse-position" || maybe.type === "auto-animate";
}
var METADATA_PROPS = new Set([
	"opacity",
	"blendMode",
	"visible",
	"transform",
	"boundingBox",
	"maskSource",
	"maskType",
	"flow",
	"absolute"
]);
async function createShader(canvas, preset, options) {
	const componentRegistry = /* @__PURE__ */ new Map();
	getAllShaders().forEach((shader) => {
		componentRegistry.set(shader.definition.name, shader.definition);
		for (const oldName of shader.definition.deprecatedNames ?? []) componentRegistry.set(oldName, shader.definition);
	});
	for (const custom of options?.components ?? []) componentRegistry.set(custom.name, custom);
	if (!canvas.style.width || !canvas.style.height) {
		const rect = canvas.getBoundingClientRect();
		const w = rect.width > 0 ? rect.width : canvas.width;
		const h = rect.height > 0 ? rect.height : canvas.height;
		if (w > 0 && !canvas.style.width) canvas.style.width = `${w}px`;
		if (h > 0 && !canvas.style.height) canvas.style.height = `${h}px`;
	}
	function cloneComponent(c) {
		return {
			...c,
			props: c.props ? { ...c.props } : void 0,
			children: c.children?.map(cloneComponent)
		};
	}
	const livePreset = {
		...preset,
		components: preset.components.map(cloneComponent)
	};
	let current = null;
	let destroyed = false;
	let rebuilding = false;
	const MAX_DEVICE_LOSS_REBUILDS = 3;
	const DEVICE_LOSS_BURST_WINDOW_MS = 6e4;
	let rebuildAttempts = 0;
	let lastDeviceLossAt = 0;
	let unavailable = null;
	function notify(reason) {
		try {
			options?.onError?.(reason);
		} catch (e) {
			debugError("[createShader] onError callback threw:", e);
		}
	}
	let reportedTerminal = false;
	function goDown(reason) {
		if (reportedTerminal) return;
		reportedTerminal = true;
		unavailable = reason;
		notify(reason);
	}
	function registerComponent(renderer, nodeEntryMap, component, parentId, renderOrder) {
		const componentDef = componentRegistry.get(component.type) ?? getRegisteredShader(component.type);
		if (!componentDef) {
			console.warn(`[createShader] Unknown component type: ${component.type}`);
			return;
		}
		const nodeId = component.id || `${component.type}_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;
		const mapsFromProps = {};
		for (const [key, val] of Object.entries(component.props || {})) if (!METADATA_PROPS.has(key) && Object.prototype.hasOwnProperty.call(componentDef.props, key) && isPropDriver(val)) mapsFromProps[key] = val;
		const uniforms = createGpuUniformsMap(componentDef, Object.fromEntries(Object.entries(componentDef.props).map(([key, propConfig]) => {
			const val = component.props?.[key] !== void 0 ? component.props[key] : propConfig.default;
			return [key, isPropDriver(val) ? propConfig.default : val];
		})), nodeId);
		const metadata = {
			blendMode: component.props?.blendMode || "normal",
			opacity: component.props?.opacity,
			visible: component.props?.visible,
			renderOrder,
			id: component.id,
			maps: Object.keys(mapsFromProps).length > 0 ? { ...mapsFromProps } : void 0,
			mask: component.props?.maskSource ? {
				source: component.props.maskSource,
				type: component.props.maskType || "alpha"
			} : void 0,
			transform: component.props?.transform ? {
				offsetX: 0,
				offsetY: 0,
				rotation: 0,
				scale: 1,
				anchorX: .5,
				anchorY: .5,
				edges: "transparent",
				...component.props.transform
			} : void 0,
			boundingBox: resolveBoundingBox(component.props?.boundingBox),
			flow: component.props?.flow,
			absolute: component.props?.absolute
		};
		renderer.registerNode(nodeId, componentDef.fragment, parentId, metadata, uniforms, componentDef);
		nodeEntryMap.set(nodeId, {
			componentDef,
			uniforms,
			nodeId,
			component,
			currentMaps: { ...mapsFromProps }
		});
		component.children?.forEach((child, index) => {
			registerComponent(renderer, nodeEntryMap, child, nodeId, index);
		});
	}
	async function build() {
		const renderer = shaderRendererGPU();
		const nodeEntryMap = /* @__PURE__ */ new Map();
		if (options?.onReady) renderer.setOnReady(options.onReady);
		renderer.setOnUnavailable((reason) => {
			if (destroyed) return;
			goDown(reason);
		});
		await renderer.initialize({
			canvas,
			resizeTarget: canvas,
			enablePerformanceTracking: options?.enablePerformanceTracking || false,
			colorSpace: options?.colorSpace,
			toneMapping: options?.toneMapping,
			observeElement: options?.observeElement,
			gpu: options?.gpu
		});
		const state = {
			renderer,
			nodeEntryMap,
			telemetryCollector: null,
			telemetryStartTimeout: null
		};
		renderer.setOnDeviceLost((reason) => {
			if (destroyed || rebuilding || unavailable) return;
			if (current !== null && current !== state) return;
			rebuilding = true;
			notify(reason);
			const now = Date.now();
			if (now - lastDeviceLossAt > DEVICE_LOSS_BURST_WINDOW_MS) rebuildAttempts = 0;
			lastDeviceLossAt = now;
			if (++rebuildAttempts > MAX_DEVICE_LOSS_REBUILDS) {
				teardownState(state);
				current = null;
				rebuilding = false;
				goDown("unrecoverable");
				return;
			}
			(async () => {
				try {
					teardownState(state);
					current = null;
					if (destroyed) return;
					const next = await build();
					if (destroyed) {
						teardownState(next);
						return;
					}
					current = next;
				} catch (rebuildErr) {
					debugError("[createShader] rebuild after device loss failed:", rebuildErr);
					goDown("rebuild_failed");
				} finally {
					rebuilding = false;
				}
			})();
		});
		const rootId = "shader-root";
		renderer.registerNode(rootId, rootPassthrough.fragment, null, null, {}, rootPassthrough);
		livePreset.components.forEach((component, index) => {
			registerComponent(renderer, nodeEntryMap, component, rootId, index);
		});
		if (isExternalUser()) {
			const MAX_TELEMETRY_POLLS = 40;
			let telemetryPolls = 0;
			const checkRendering = () => {
				if (destroyed || unavailable || current !== state || ++telemetryPolls > MAX_TELEMETRY_POLLS) {
					state.telemetryStartTimeout = null;
					return;
				}
				if (renderer.getPerformanceStats().fps > 0) {
					state.telemetryCollector = startTelemetry(renderer, "4.0.2", options?.disableTelemetry || false, false);
					if (state.telemetryCollector) state.telemetryCollector.start();
					state.telemetryStartTimeout = null;
				} else state.telemetryStartTimeout = setTimeout(checkRendering, 500);
			};
			state.telemetryStartTimeout = setTimeout(checkRendering, 500);
		}
		return state;
	}
	function teardownState(state) {
		if (state.telemetryCollector) {
			try {
				state.telemetryCollector.stop();
			} catch {}
			state.telemetryCollector = null;
		}
		if (state.telemetryStartTimeout !== null) {
			clearTimeout(state.telemetryStartTimeout);
			state.telemetryStartTimeout = null;
		}
		try {
			state.renderer.cleanup();
		} catch {}
	}
	current = await build();
	function update(componentId, props) {
		if (!current) return;
		const { renderer, nodeEntryMap } = current;
		const entry = nodeEntryMap.get(componentId);
		if (!entry) {
			console.warn(`[createShader] Component ID not found: ${componentId}`);
			return;
		}
		if (!entry.component.props) entry.component.props = {};
		const live = entry.component.props;
		for (const [key, value] of Object.entries(props)) if (METADATA_PROPS.has(key)) if (key === "maskSource") {
			renderer.updateNodeMetadata(entry.nodeId, { mask: value ? {
				source: value,
				type: props.maskType || live.maskType || "alpha"
			} : void 0 });
			live.maskSource = value;
			if ("maskType" in props) live.maskType = props.maskType;
		} else if (key === "maskType") continue;
		else if (key === "transform") {
			renderer.updateNodeMetadata(entry.nodeId, { transform: value });
			live.transform = value;
		} else if (key === "boundingBox") {
			renderer.updateNodeMetadata(entry.nodeId, { boundingBox: resolveBoundingBox(value) });
			live.boundingBox = value;
		} else {
			renderer.updateNodeMetadata(entry.nodeId, { [key]: value });
			live[key] = value;
		}
		else if (isPropDriver(value)) if (!Object.prototype.hasOwnProperty.call(entry.componentDef.props, key)) console.warn(`[createShader] Ignoring PropDriver for unknown prop "${key}" on ${entry.component.type}`);
		else {
			entry.currentMaps[key] = value;
			renderer.updateNodeMetadata(entry.nodeId, { maps: { ...entry.currentMaps } });
			live[key] = value;
		}
		else {
			if (key in entry.currentMaps) {
				delete entry.currentMaps[key];
				renderer.updateNodeMetadata(entry.nodeId, { maps: Object.keys(entry.currentMaps).length > 0 ? { ...entry.currentMaps } : void 0 });
			}
			if (Object.prototype.hasOwnProperty.call(entry.componentDef.props, key)) {
				renderer.updateUniformValue(entry.nodeId, key, value);
				live[key] = value;
			}
		}
	}
	function resize(width, height) {
		const rect = canvas.getBoundingClientRect();
		const w = width ?? (rect.width > 0 ? rect.width : canvas.width);
		const h = height ?? (rect.height > 0 ? rect.height : canvas.height);
		if (w > 0 && h > 0) {
			canvas.style.width = `${w}px`;
			canvas.style.height = `${h}px`;
			if (current) current.renderer.resize(w, h);
		}
	}
	function destroy() {
		destroyed = true;
		if (current) {
			teardownState(current);
			current = null;
		}
	}
	function pause() {
		if (current) current.renderer.stopAnimation();
	}
	function resume() {
		if (current) current.renderer.startAnimation();
	}
	return {
		update,
		resize,
		pause,
		resume,
		destroy,
		getFailureReason: () => unavailable
	};
}
export { createShader };
