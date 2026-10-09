import { Ac as defineStd, Bd as ZERO, Dc as vec3, Fd as applyEdgeHandlingExpr, Gs as clamp, Hd as call, Id as bSplineTapSetup, Js as dot, Tc as sub, dc as mul, id as toHalfFloat, if as unpremultiplyAlpha, nt as hostGridProgram, oc as local, rt as hostStep, tt as hostFieldTexture, uc as mix } from "./std-ClftfWpF.js";
import { s as transformEdges } from "./transformations-P50Uo_Ju.js";
var transformThrowKey = (value) => {
	return {
		luminance: 0,
		darkness: 1,
		red: 2,
		green: 3,
		blue: 4
	}[value] ?? 0;
};
var GRID = 128;
var num = (v, f) => typeof v === "number" ? v : f;
var offsetUV = (uv, flowVec, scale) => sub(uv, mul(flowVec, scale));
var { Taps: PixelThrowBicubicTaps, setup: pixelThrowBicubicSetup } = bSplineTapSetup(GRID, {
	fnName: "pixelThrowBicubicSetup",
	structName: "PixelThrowBicubicTaps"
});
function keyScaleOf(keyMode, rgb, keyInfluence) {
	const lum = vec3(.299, .587, .114);
	return mix(1, clamp(keyMode === 1 ? sub(1, dot(rgb, lum)) : keyMode === 2 ? rgb.member("r") : keyMode === 3 ? rgb.member("g") : keyMode === 4 ? rgb.member("b") : dot(rgb, lum), 0, 1), keyInfluence);
}
const componentDefinition = defineStd({
	name: "PixelThrow",
	role: "simulation",
	species: "custom",
	category: "Interactive",
	description: "Throws pixels along the cursor's path like a fluid — brighter (or redder/greener/bluer) pixels are flung farther, then settle back with friction",
	requiresRTT: true,
	requiresChild: true,
	boundingBoxDeclaration: { aspectRatio: null },
	usesPointer: true,
	props: {
		throwKey: {
			default: "luminance",
			transform: transformThrowKey,
			compileTime: true,
			description: "Which pixel value decides how far a pixel is thrown",
			ui: {
				type: "select",
				options: [
					{
						label: "Brightness",
						value: "luminance"
					},
					{
						label: "Darkness",
						value: "darkness"
					},
					{
						label: "Red",
						value: "red"
					},
					{
						label: "Green",
						value: "green"
					},
					{
						label: "Blue",
						value: "blue"
					}
				],
				label: "Throw By",
				group: "Effect"
			}
		},
		strength: {
			default: .25,
			description: "Maximum distance pixels can be thrown",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		keyInfluence: {
			default: .8,
			description: "How much the chosen value modulates throw distance (0 = all pixels thrown equally, 1 = only high-value pixels move)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Key Influence",
				group: "Effect"
			}
		},
		radius: {
			default: .2,
			description: "Size of the cursor's throw brush",
			ui: {
				type: ["range", "map"],
				min: .02,
				max: .6,
				step: .01,
				label: "Radius",
				group: "Effect"
			}
		},
		friction: {
			default: .3,
			description: "How quickly thrown pixels settle back to their origin (0 = float almost forever, 1 = snap back fast)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Friction",
				group: "Effect"
			}
		},
		momentum: {
			default: .5,
			description: "How much the thrown flow keeps drifting and swirling after the cursor passes (fluidity)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Momentum",
				group: "Effect"
			}
		},
		edges: {
			default: "stretch",
			description: "How to handle pixels thrown beyond the canvas edges",
			transform: transformEdges,
			compileTime: true,
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
					},
					{
						label: "Transparent",
						value: "transparent"
					},
					{
						label: "Mirror",
						value: "mirror"
					},
					{
						label: "Wrap",
						value: "wrap"
					}
				],
				label: "Edges",
				group: "Effect"
			}
		}
	},
	gpu: { fragment: (params) => {
		const { uniforms, ctx, childNode, convertToTexture, onBeforeRender, dimensions, propValues, getCpuValue } = params;
		if (!childNode) return ZERO;
		const child = convertToTexture(childNode);
		const edgeMode = propValues.edges ?? 0;
		const rawKey = propValues.throwKey;
		const keyMode = typeof rawKey === "number" ? rawKey : transformThrowKey(String(rawKey ?? "luminance"));
		const flow = new Float32Array(GRID * GRID * 2);
		const temp = new Float32Array(GRID * GRID * 2);
		const field = hostFieldTexture(params, {
			size: GRID,
			label: "pixelthrow-flow"
		});
		const flowKit = field.texture;
		let prevX = .5, prevY = .5;
		let velX = 0, velY = 0;
		let speed = 0;
		let maxThrow = .125;
		const sampleFlow = (gx, gy, ch) => {
			const x0 = Math.floor(gx), y0 = Math.floor(gy);
			const x1 = x0 + 1, y1 = y0 + 1;
			if (x0 < 0 || y0 < 0 || x1 >= GRID || y1 >= GRID) return 0;
			const fx = gx - x0, fy = gy - y0;
			const i00 = (y0 * GRID + x0) * 2 + ch;
			const i01 = (y0 * GRID + x1) * 2 + ch;
			const i10 = (y1 * GRID + x0) * 2 + ch;
			const i11 = (y1 * GRID + x1) * 2 + ch;
			return flow[i00] * (1 - fx) * (1 - fy) + flow[i01] * fx * (1 - fy) + flow[i10] * (1 - fx) * fy + flow[i11] * fx * fy;
		};
		const aspectOf = () => Math.max(1e-4, dimensions.width / Math.max(1, dimensions.height));
		onBeforeRender(hostGridProgram({
			clampDt: .016,
			steps: [
				hostStep("dtGuard", (f) => f.dt <= 0 ? "skip" : void 0),
				hostStep("cursorVelocity", (f) => {
					const rawVelX = (f.pointer.x - prevX) / f.dt;
					const rawVelY = (f.pointer.y - prevY) / f.dt;
					velX = velX * .8 + rawVelX * .2;
					velY = velY * .8 + rawVelY * .2;
					speed = Math.sqrt(velX * velX + velY * velY);
				}),
				hostStep("advect+decay", (f) => {
					const friction = num(getCpuValue("friction"), .3);
					const momentum = num(getCpuValue("momentum"), .5);
					maxThrow = num(getCpuValue("strength"), .25) * .5;
					const decay = Math.max(0, 1 - friction * f.dt * 4);
					const advect = momentum * f.dt * 12;
					for (let i = 0; i < GRID; i++) for (let j = 0; j < GRID; j++) {
						const idx = (i * GRID + j) * 2;
						const srcX = j - flow[idx] * GRID * advect;
						const srcY = i - flow[idx + 1] * GRID * advect;
						temp[idx] = sampleFlow(srcX, srcY, 0) * decay;
						temp[idx + 1] = sampleFlow(srcX, srcY, 1) * decay;
					}
				}),
				hostStep("inject(brush)", (f) => {
					if (speed <= .02) return;
					const aspect = aspectOf();
					const r = Math.max(.001, num(getCpuValue("radius"), .2));
					const r2 = r * r;
					const EDGE = Math.exp(-1);
					const INV_EDGE = 1 / (1 - EDGE);
					const minI = Math.max(0, Math.floor((f.pointer.y - r) * GRID));
					const maxI = Math.min(GRID - 1, Math.ceil((f.pointer.y + r) * GRID));
					const minJ = Math.max(0, Math.floor((f.pointer.x - r) * GRID));
					const maxJ = Math.min(GRID - 1, Math.ceil((f.pointer.x + r) * GRID));
					for (let i = minI; i <= maxI; i++) for (let j = minJ; j <= maxJ; j++) {
						const cellX = (j + .5) / GRID;
						const cellY = (i + .5) / GRID;
						const dx = aspect >= 1 ? (cellX - f.pointer.x) * aspect : cellX - f.pointer.x;
						const dy = aspect >= 1 ? cellY - f.pointer.y : (cellY - f.pointer.y) / aspect;
						const distSq = dx * dx + dy * dy;
						if (distSq > r2) continue;
						const influence = (Math.exp(-distSq / r2) - EDGE) * INV_EDGE;
						const idx = (i * GRID + j) * 2;
						temp[idx] += velX * influence * f.dt * 4;
						temp[idx + 1] += velY * influence * f.dt * 4;
					}
				}),
				hostStep("saturate(tanh)", () => {
					for (let k = 0; k < GRID * GRID; k++) {
						const fx = temp[k * 2];
						const fy = temp[k * 2 + 1];
						const len = Math.sqrt(fx * fx + fy * fy);
						if (len > 1e-6) {
							const s = maxThrow * Math.tanh(len / maxThrow) / len;
							temp[k * 2] = fx * s;
							temp[k * 2 + 1] = fy * s;
						}
					}
				}),
				hostStep("publish(halfFloat)", (f) => {
					flow.set(temp);
					for (let k = 0; k < GRID * GRID; k++) {
						field.texData[k * 4] = toHalfFloat(flow[k * 2]);
						field.texData[k * 4 + 1] = toHalfFloat(flow[k * 2 + 1]);
					}
					field.upload();
					prevX = f.pointer.x;
					prevY = f.pointer.y;
				})
			]
		}));
		const uv = ctx.uv;
		const taps = call(pixelThrowBicubicSetup, "pixelThrowBicubicSetup", [uv]);
		const tapW = taps.member("w");
		const flowVec = local(flowKit.sample(taps.member("uvA"), "linearClamp").member("xy").mul(tapW.member("x")).add(flowKit.sample(taps.member("uvB"), "linearClamp").member("xy").mul(tapW.member("y"))).add(flowKit.sample(taps.member("uvC"), "linearClamp").member("xy").mul(tapW.member("z"))).add(flowKit.sample(taps.member("uvD"), "linearClamp").member("xy").mul(tapW.member("w"))), "flowVec");
		const probeUV = offsetUV(uv, flowVec, .5);
		return call(unpremultiplyAlpha, "unpremultiplyAlpha", [applyEdgeHandlingExpr(local(offsetUV(uv, flowVec, keyScaleOf(keyMode, call(unpremultiplyAlpha, "unpremultiplyAlpha", [child.sample(probeUV)]).member("rgb"), uniforms.keyInfluence)), "displacedUV"), (u) => child.sample(u), edgeMode)]);
	} }
});
var PixelThrow_default = componentDefinition;
export { pixelThrowBicubicSetup as i, PixelThrow_default as n, componentDefinition as r, PixelThrowBicubicTaps as t };
