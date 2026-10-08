import { Ac as defineStd, Bd as ZERO, Ed as mixColorStopsRuntime, Hd as call, Jd as vec4, af as colorMixing_exports, id as toHalfFloat, od as createPointerVelocityTracker, ou as colorStopsPropConfig, pu as makeStrokePolylineScan } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var MAX_POINTS = 64;
var HALF_ONE = toHalfFloat(1);
var WIDTH_MIN = .2;
var SPEED_FULL = 1.5;
const cursorTrailScan = makeStrokePolylineScan({
	maxPoints: MAX_POINTS,
	subdivisions: 3,
	name: "cursorTrailScan"
});
function polylineRecorder(params) {
	const { createDataTexture, registerMediaTexture, onBeforeRender, onCleanup, onResize, getCpuValue, dimensions } = params;
	const pts = [];
	const texData = new Uint16Array(MAX_POINTS * 4);
	for (let i = 0; i < MAX_POINTS; i++) texData[i * 4 + 2] = HALF_ONE;
	const dataTex = createDataTexture({
		width: MAX_POINTS,
		height: 1,
		format: "rgba16float",
		data: texData,
		label: "cursortrail-points"
	});
	onCleanup(() => dataTex.destroy());
	const pointsKit = registerMediaTexture(() => dataTex.texture);
	const tracker = createPointerVelocityTracker({
		minDrag: .001,
		teleportGuard: Number.POSITIVE_INFINITY
	});
	let curW = Math.max(1, Math.round(dimensions.width));
	let curH = Math.max(1, Math.round(dimensions.height));
	onResize(({ width, height }) => {
		curW = Math.max(1, Math.round(width));
		curH = Math.max(1, Math.round(height));
	});
	let headDist = 0;
	let wasActive = false;
	onBeforeRender((fp) => {
		const { pointer, deltaTime } = fp;
		const rawDt = Math.max(0, deltaTime ?? 0);
		const dt = Math.min(rawDt, .1);
		const length = getCpuValue("length") ?? .5;
		const drawRadius = (getCpuValue("radius") ?? .5) * .1;
		const aspect = curW / curH;
		const move = tracker.update(pointer, dt);
		const ageRate = Math.min(rawDt, .25) / Math.max(.1, length);
		for (const p of pts) p.age = Math.min(1, p.age + ageRate);
		while (pts.length > 0 && pts[0].age >= 1) pts.shift();
		if (move.dragDist > .001) {
			const stepDist = Math.sqrt(move.dx * aspect * (move.dx * aspect) + move.dy * move.dy);
			const widthNow = Math.min(1, Math.max(WIDTH_MIN, Math.sqrt(move.smoothSpeed / SPEED_FULL)));
			if (pts.length === 0) {
				pts.push({
					x: move.prevX,
					y: move.prevY,
					age: 0,
					link: 0,
					width: widthNow
				});
				pts.push({
					x: move.x,
					y: move.y,
					age: 0,
					link: 1,
					width: widthNow
				});
				headDist = stepDist;
			} else {
				const head = pts[pts.length - 1];
				head.x = move.x;
				head.y = move.y;
				head.age = 0;
				head.width = widthNow;
				headDist += stepDist;
				if (headDist >= Math.max(.006, drawRadius * .5)) {
					const n = pts.length;
					if (n >= 3 && pts[n - 2].link === 1) {
						pts[n - 2].x = .25 * pts[n - 3].x + .5 * pts[n - 2].x + .25 * pts[n - 1].x;
						pts[n - 2].y = .25 * pts[n - 3].y + .5 * pts[n - 2].y + .25 * pts[n - 1].y;
					}
					pts.push({
						x: move.x,
						y: move.y,
						age: 0,
						link: 1,
						width: widthNow
					});
					headDist = 0;
				}
			}
			while (pts.length > MAX_POINTS - 1) pts.shift();
		}
		const active = pts.length > 0;
		if (active || wasActive) {
			const count = pts.length > 0 ? pts.length + 1 : 0;
			for (let i = 0; i < MAX_POINTS; i++) {
				const base = i * 4;
				if (i < count) {
					const p = pts[Math.min(i, pts.length - 1)];
					texData[base] = toHalfFloat(p.x);
					texData[base + 1] = toHalfFloat(p.y);
					texData[base + 2] = toHalfFloat(p.age);
					texData[base + 3] = toHalfFloat(i === pts.length || p.link === 1 ? p.width : -p.width);
				} else {
					texData[base] = 0;
					texData[base + 1] = 0;
					texData[base + 2] = HALF_ONE;
					texData[base + 3] = 0;
				}
			}
			dataTex.write(texData);
		}
		wasActive = active;
	});
	return pointsKit;
}
function ageColorRamp(params, trailAge) {
	const { propValues, uniforms } = params;
	const colorSpaceMode = propValues.colorSpace ?? 0;
	if ((propValues.stopCount ?? 0) > 1) return mixColorStopsRuntime(trailAge, {
		colorsArray: uniforms.colorsArray,
		positionsArray: uniforms.positionsArray,
		convertedColorsArray: uniforms.convertedColorsArray,
		stopCount: uniforms.stopCount
	}, colorSpaceMode);
	const cm = colorMixing_exports;
	return call(cm.mixColorsVariants[colorSpaceMode] ?? cm.mixColorsLinear, "mixColors", [
		uniforms.colorA,
		uniforms.colorB,
		trailAge
	]);
}
const componentDefinition = defineStd({
	name: "CursorTrail",
	role: "simulation",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Animated trail effect that tracks cursor movement",
	usesPointer: true,
	props: {
		colorA: {
			default: "#00aaff",
			transform: transformColor,
			description: "Color of fresh trails",
			ui: {
				type: "color",
				label: "Start Color",
				group: "Colors"
			}
		},
		colorB: {
			default: "#ff00aa",
			transform: transformColor,
			description: "Color trails transition to as they fade",
			ui: {
				type: "color",
				label: "End Color",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		radius: {
			default: .5,
			description: "Base radius of the trail stroke",
			ui: {
				type: "range",
				min: .5,
				max: 2,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		length: {
			default: .5,
			description: "How long the trail persists (in seconds)",
			ui: {
				type: "range",
				min: .1,
				max: 2,
				step: .1,
				label: "Trail Length",
				group: "Animation"
			}
		},
		shrink: {
			default: 1,
			description: "How much the stroke tapers as it fades out (0 = no taper, 1 = full taper)",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .1,
				label: "Shrink Amount",
				group: "Effect"
			}
		},
		softness: {
			default: 0,
			description: "Edge softness of the trail (0 = crisp paint edge, 1 = very soft)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	},
	gpu: { fragment: (params) => {
		const { ctx, uniforms, gpu } = params;
		if (!gpu?.root) return ZERO;
		const scan = call(cursorTrailScan, "cursorTrailScan", [
			polylineRecorder(params).accessor(),
			ctx.uv,
			ctx.aspect,
			uniforms.radius,
			uniforms.shrink,
			uniforms.softness,
			ctx.viewportSize.member("y")
		]);
		const trailAge = scan.member("x");
		const coverage = scan.member("y");
		const trailColor = ageColorRamp(params, trailAge);
		return vec4(trailColor.member("rgb"), trailColor.member("a").mul(coverage));
	} }
});
var CursorTrail_default = componentDefinition;
export { componentDefinition as n, cursorTrailScan as r, CursorTrail_default as t };
