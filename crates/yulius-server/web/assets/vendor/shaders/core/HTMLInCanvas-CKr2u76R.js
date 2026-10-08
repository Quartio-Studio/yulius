import { Ac as defineStd, Hd as call, Jd as vec4, tf as srgbToLinear, yd as createSwappableMediaTexture } from "./std-ClftfWpF.js";
function createDomCaptureHost(params) {
	const { gpu, domCanvas, onBeforeRender, onResize, onCleanup } = params;
	const device = gpu.device;
	const initialDpr = Math.min(window.devicePixelRatio, 2);
	const tex = createSwappableMediaTexture(params, {
		label: "HTMLInCanvas",
		initial: {
			width: Math.max(1, Math.round(window.innerWidth * initialDpr)),
			height: Math.max(1, Math.round(window.innerHeight * initialDpr))
		},
		mipmaps: false
	});
	let contentElement = null;
	let hasLoggedError = false;
	let paintSetup = false;
	let hasFreshSnapshot = false;
	let useLegacySignature = false;
	const ensureTexSize = (w, h) => tex.ensureSize(w, h);
	onBeforeRender(() => {
		if (!domCanvas || tex.disposed) return;
		if (!contentElement) {
			contentElement = domCanvas.firstElementChild;
			if (contentElement) {
				const rect = contentElement.getBoundingClientRect();
				const physW = Math.round(rect.width * window.devicePixelRatio);
				const physH = Math.round(rect.height * window.devicePixelRatio);
				if (physW > 0 && physH > 0) ensureTexSize(physW, physH);
			}
		}
		if (!contentElement) return;
		if (!paintSetup) {
			domCanvas.getContext("2d");
			domCanvas.onpaint = () => {
				hasFreshSnapshot = true;
			};
			domCanvas.requestPaint?.();
			paintSetup = true;
			return;
		}
		if (hasFreshSnapshot && contentElement) {
			const copyFn = device.queue.copyElementImageToTexture;
			if (typeof copyFn !== "function") {
				hasFreshSnapshot = false;
				return;
			}
			try {
				if (useLegacySignature) copyFn.call(device.queue, contentElement, { texture: tex.unwrap() });
				else try {
					copyFn.call(device.queue, { source: contentElement }, { destination: { texture: tex.unwrap() } });
				} catch (e) {
					if (!(e instanceof TypeError)) throw e;
					copyFn.call(device.queue, contentElement, { texture: tex.unwrap() });
					useLegacySignature = true;
				}
				hasFreshSnapshot = false;
			} catch (e) {
				if (!hasLoggedError) {
					console.warn("[HTMLInCanvas] copyElementImageToTexture failed.\nRequires Chrome Canary with chrome://flags/#canvas-draw-element enabled.\nThe element must be a direct child of a <canvas layoutsubtree>.", e);
					hasLoggedError = true;
				}
			}
		}
	});
	onResize(() => {
		const newDpr = Math.min(window.devicePixelRatio, 2);
		ensureTexSize(Math.round(window.innerWidth * newDpr), Math.round(window.innerHeight * newDpr));
		contentElement = null;
		hasFreshSnapshot = false;
		if (domCanvas) domCanvas.requestPaint?.();
	});
	onCleanup(() => {
		if (domCanvas) domCanvas.onpaint = null;
	});
	return tex;
}
const componentDefinition = defineStd({
	name: "HTMLInCanvas",
	role: "media",
	species: "custom",
	deprecatedNames: ["DOMTexture"],
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Textures",
	description: "Render live HTML/DOM content as a WebGPU texture layer via the html-in-canvas API. Requires Chrome Canary with chrome://flags/#canvas-draw-element enabled.",
	capturesDOM: true,
	experimental: {
		message: "Requires Chrome Canary with chrome://flags/#canvas-draw-element enabled. Do not use for production use.",
		docs: "This component is powered by the WICG html-in-canvas proposal, which is currently only available in Chrome Canary behind a feature flag. It is not suitable for production use and may change as the specification evolves.",
		docsLink: {
			url: "https://github.com/WICG/html-in-canvas",
			label: "View the spec"
		}
	},
	props: {},
	gpu: { fragment: (params) => {
		const sampled = createDomCaptureHost(params).kit.sample(params.ctx.uv);
		return vec4(call(srgbToLinear, "srgbToLinear", [sampled.member("rgb")]), sampled.member("a"));
	} }
});
var HTMLInCanvas_default = componentDefinition;
export { componentDefinition as n, HTMLInCanvas_default as t };
