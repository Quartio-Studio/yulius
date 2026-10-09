import { Ac as defineStd, Bd as ZERO, Dc as vec3, Hd as call, Js as dot, Kd as floatE, Oc as vec4, cc as max, dc as mul, fd as isMobileGpuViewport, g as feedbackSim, if as unpremultiplyAlpha, nf as blendModes, oc as local, qs as div, zc as buildFeedbackTrailStep } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
var STATE_RES = isMobileGpuViewport() ? 384 : 640;
var STATE_FORMAT = "rgba16float";
var DIFFUSE_HALF_UV = 2.5 * .5 / STATE_RES;
var DRIFT_SCALE = .5;
var RAINBOW_RATE = .35;
var TINT_MODES = {
	none: 0,
	color: 1,
	rainbow: 2
};
var SOURCE_MODES = {
	motion: 1,
	alpha: 0
};
var buildTrailKernel = (source, hueCycle) => buildFeedbackTrailStep({
	res: STATE_RES,
	format: STATE_FORMAT,
	namePrefix: "timeTrail",
	source,
	hueCycle,
	diffuseHalfUv: DIFFUSE_HALF_UV,
	driftScale: DRIFT_SCALE
});
const timeTrailKernel = buildTrailKernel("motion", false).kernel;
var resolveBlendKey = (v) => {
	const key = v === "add" ? "linearDodge" : String(v ?? "normal");
	return key in blendModes ? key : "normal";
};
var stampModeFor = (key) => key === "screen" ? 1 : key === "linearDodge" ? 2 : 0;
const componentDefinition = defineStd({
	name: "TimeTrail",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "An Echo-style temporal trail — whatever moves sheds a smooth, decaying long-exposure ghost trail (frame-difference Motion mode works on opaque images and video; Alpha mode trails a shape's coverage), with optional zoom-feedback tunnels and rainbow light-painting",
	requiresRTT: true,
	requiresChild: true,
	props: {
		trailSource: {
			default: "motion",
			description: "What sheds the trail — Motion trails whatever changes frame-to-frame (works on opaque images and video), Alpha trails the child's transparency coverage (best for a clean shape over empty canvas)",
			compileTime: true,
			transform: (v) => SOURCE_MODES[v] ?? 1,
			ui: {
				type: "select",
				options: [{
					label: "Motion",
					value: "motion"
				}, {
					label: "Alpha",
					value: "alpha"
				}],
				label: "Source",
				group: "Trail"
			}
		},
		motionThreshold: {
			default: .06,
			description: "How much a pixel must change between frames to shed a trail (Motion source only) — lower catches subtle movement, higher only fast motion",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: .5,
				step: .01,
				label: "Motion Threshold",
				group: "Trail"
			}
		},
		trailLength: {
			default: .6,
			description: "How long the trail lingers, in seconds — the time constant of the fade (the star control)",
			ui: {
				type: ["range", "map"],
				min: .05,
				max: 4,
				step: .01,
				label: "Trail Length",
				group: "Trail"
			}
		},
		diffusion: {
			default: 1,
			description: "How much the trail smears and softens as it fades — 0 keeps crisp ghost frames, higher melts them together",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Diffusion",
				group: "Trail"
			}
		},
		trailOpacity: {
			default: 1,
			description: "Overall strength of the trail behind the live frame",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Trail Opacity",
				group: "Trail"
			}
		},
		trailBlend: {
			default: "normal",
			description: "How the trail combines with the frame — Add / Screen build glowing light-painting trails, Multiply lays down dark ink ghosts, plus the full standard blend set",
			compileTime: true,
			ui: {
				type: "select",
				options: [
					{
						label: "Normal",
						value: "normal"
					},
					{
						label: "Add",
						value: "add"
					},
					{
						label: "Screen",
						value: "screen"
					},
					{
						label: "Multiply",
						value: "multiply"
					},
					{
						label: "Overlay",
						value: "overlay"
					},
					{
						label: "Soft Light",
						value: "softLight"
					},
					{
						label: "Hard Light",
						value: "hardLight"
					},
					{
						label: "Darken",
						value: "darken"
					},
					{
						label: "Lighten",
						value: "lighten"
					},
					{
						label: "Color Dodge",
						value: "colorDodge"
					},
					{
						label: "Color Burn",
						value: "colorBurn"
					},
					{
						label: "Linear Burn",
						value: "linearBurn"
					},
					{
						label: "Difference",
						value: "difference"
					},
					{
						label: "Exclusion",
						value: "exclusion"
					},
					{
						label: "Hue",
						value: "hue"
					},
					{
						label: "Saturation",
						value: "saturation"
					},
					{
						label: "Color",
						value: "color"
					},
					{
						label: "Luminosity",
						value: "luminosity"
					}
				],
				label: "Blend",
				group: "Trail"
			}
		},
		driftX: {
			default: 0,
			description: "Sideways drift of the trail per second — smears the trail in a direction",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Drift X",
				group: "Feedback"
			}
		},
		driftY: {
			default: 0,
			description: "Vertical drift of the trail per second",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .01,
				label: "Drift Y",
				group: "Feedback"
			}
		},
		zoom: {
			default: 1,
			description: "Per-frame zoom of the trail around the centre — above 1 pulls the trail into an outward tunnel, below 1 sucks it inward",
			ui: {
				type: ["range", "map"],
				min: .9,
				max: 1.1,
				step: .001,
				label: "Zoom",
				group: "Feedback"
			}
		},
		tintMode: {
			default: "none",
			description: "color the trail — Color tints it a single hue, Rainbow cycles hue as the trail ages",
			compileTime: true,
			transform: (v) => TINT_MODES[v] ?? 0,
			ui: {
				type: "select",
				options: [
					{
						label: "None",
						value: "none"
					},
					{
						label: "Color",
						value: "color"
					},
					{
						label: "Rainbow",
						value: "rainbow"
					}
				],
				label: "Tint Mode",
				group: "Color"
			}
		},
		tint: {
			default: "#33aaff",
			transform: transformColor,
			description: "Trail tint color (used in Color mode)",
			ui: {
				type: "color",
				label: "Tint",
				group: "Color"
			}
		},
		speed: {
			default: 1,
			description: "Simulation speed. 0 freezes the trail in place.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		}
	},
	...feedbackSim((params, root) => {
		const { getCpuValue, propValues } = params;
		const tintMode = Number(propValues.tintMode) || 0;
		const blendMode = stampModeFor(resolveBlendKey(propValues.trailBlend));
		const isMotion = (propValues.trailSource == null ? 1 : Number(propValues.trailSource)) > .5;
		const trailSampler = root.createSampler({
			magFilter: "linear",
			minFilter: "linear",
			addressModeU: "clamp-to-edge",
			addressModeV: "clamp-to-edge"
		});
		const trail = buildTrailKernel(isMotion ? "motion" : "alpha", tintMode === 2);
		return {
			size: STATE_RES,
			format: STATE_FORMAT,
			extraSlots: { prevLive: isMotion ? STATE_RES : [1, 1] },
			speedProp: "speed",
			paramsSchema: trail.Params,
			bindGroups: ({ childTexture, display, paramsBuffer }, read, write) => root.createBindGroup(trail.layout, {
				src: childTexture,
				prev: read.state,
				next: write.state,
				display,
				prevLive: read.extra.prevLive,
				prevLiveOut: write.extra.prevLive,
				samp: trailSampler,
				params: paramsBuffer
			}),
			step: trail.kernel,
			skip: () => (getCpuValue("trailOpacity") ?? 1) === 0,
			values: ({ dt }) => {
				const zoom = getCpuValue("zoom") ?? 1;
				const trailLength = Math.max(getCpuValue("trailLength") ?? .6, .001);
				return {
					persistence: Math.exp(-dt / trailLength),
					diffusion: getCpuValue("diffusion") ?? .3,
					driftX: getCpuValue("driftX") ?? 0,
					driftY: getCpuValue("driftY") ?? 0,
					dt,
					zoom: Math.pow(zoom, dt * 60),
					hueRate: tintMode === 2 ? RAINBOW_RATE * dt : 0,
					blendMode,
					motionThreshold: getCpuValue("motionThreshold") ?? .06
				};
			}
		};
	}),
	gpu: { fragment: ({ childNode, ctx, computeOutputs, uniforms, propValues, convertToTexture }) => {
		if (!childNode) return ZERO;
		const display = computeOutputs?.display;
		const live = (computeOutputs?.childTexture ?? convertToTexture(childNode)).sample(ctx.uv);
		if (!display) return call(unpremultiplyAlpha, "unpremultiplyAlpha", [live]);
		const blendKey = resolveBlendKey(propValues.trailBlend);
		const blendFn = blendModes[blendKey];
		const tintMode = Number(propValues.tintMode) || 0;
		const sourceMode = propValues.trailSource == null ? 1 : Number(propValues.trailSource);
		let trail = local(display.sample(ctx.uv, "linearClamp"), "trail");
		if (tintMode === 1) {
			const a = max(trail.member("a"), 1e-4);
			const lum = dot(div(trail.member("rgb"), a), vec3(.299, .587, .114));
			trail = local(vec4(mul(mul(uniforms.tint.member("rgb"), mul(lum, 1.5)), trail.member("a")), trail.member("a")), "tinted");
		}
		const liveStraight = call(unpremultiplyAlpha, "unpremultiplyLive", [live]);
		if (sourceMode === 1) {
			const trailStraight$1 = call(unpremultiplyAlpha, "unpremultiplyTrail", [trail]);
			return call(unpremultiplyAlpha, "unpremultiplyOut", [call(blendFn, `trailBlend_${blendKey}`, [
				liveStraight,
				trailStraight$1,
				uniforms.trailOpacity
			])]);
		}
		const trailStraight = call(unpremultiplyAlpha, "unpremultiplyTrail", [mul(trail, uniforms.trailOpacity)]);
		return call(unpremultiplyAlpha, "unpremultiplyOut", [call(blendFn, `trailBlend_${blendKey}`, [
			trailStraight,
			liveStraight,
			floatE(1)
		])]);
	} }
});
var TimeTrail_default = componentDefinition;
export { componentDefinition as n, timeTrailKernel as r, TimeTrail_default as t };
