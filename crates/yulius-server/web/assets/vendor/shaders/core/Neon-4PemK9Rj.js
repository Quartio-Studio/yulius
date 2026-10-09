import { $f as f32, Ac as defineStd, Af as exp2, Bf as sqrt, Cf as select, Df as cos, Ef as clamp, Ff as mix, Hd as call, Hf as vec2f, Kd as floatE, Lf as pow, Nf as max, Rf as sin, Ru as shapeEffectBoundingBoxDeclaration, Tf as atan2, Uf as vec3f, Wf as vec4f, bu as offsetUV, hi as shapedSurface, kf as exp, np as __export, wf as abs, xf as typegpu_default, xu as sdfSpaceUV, zf as smoothstep } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var neon_exports = /* @__PURE__ */ __export({
	applyNeonEffect: () => applyNeonEffect$1,
	neonComposite: () => neonComposite,
	neonFlicker: () => neonFlicker,
	neonFlow: () => neonFlow,
	neonGlow: () => neonGlow,
	neonSmoothEPS: () => neonSmoothEPS
});
var DEG_TO_RAD = .017453292519943295;
var _LZ = .7;
var _INV_LIGHT_LEN = 1 / Math.sqrt(1 + _LZ * _LZ);
var _LZN = _LZ * _INV_LIGHT_LEN;
var _HZ = _LZN + 1;
var _INV_H_LEN = 1 / Math.sqrt(_INV_LIGHT_LEN * _INV_LIGHT_LEN + _HZ * _HZ);
const neonSmoothEPS = typegpu_default.fn([
	f32,
	f32,
	f32
], f32)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((tubeThickness, cornerSmoothing, volumetric) => {
	"use gpu";
	const vol = max(tubeThickness, .006);
	return select(__tsover_add(.008, __tsover_mul(cornerSmoothing, .03)), vol, volumetric > .5);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "tubeThickness"
			},
			{
				"type": "i",
				"name": "cornerSmoothing"
			},
			{
				"type": "i",
				"name": "volumetric"
			}
		],
		"body": [0, [
			[
				13,
				"vol",
				[
					6,
					"std.max",
					["tubeThickness", [5, "0.006"]]
				]
			],
			[
				13,
				"flat",
				[
					1,
					[5, "0.008"],
					"+",
					[
						1,
						"cornerSmoothing",
						"*",
						[5, "0.03"]
					]
				]
			],
			[10, [
				6,
				"std.select",
				[
					"flat",
					"vol",
					[
						1,
						"volumetric",
						">",
						[5, "0.5"]
					]
				]
			]]
		]]
	},
	externals: {
		"std.max": () => max,
		"std.select": () => select
	}
}) && $.f)({}));
const neonGlow = typegpu_default.fn([
	f32,
	f32,
	f32
], f32)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((sdf, glowRadius, glowIntensity) => {
	"use gpu";
	const glowR = max(glowRadius, .001);
	const sdfOutside = max(sdf, 0);
	const innerGlow = __tsover_mul(exp(__tsover_mul(__tsover_mul(sdfOutside, -1), __tsover_div(12, glowR))), .7);
	const outerGlow = __tsover_mul(exp(__tsover_mul(__tsover_mul(sdfOutside, -1), __tsover_div(4, glowR))), .35);
	const totalGlow = __tsover_mul(__tsover_add(innerGlow, outerGlow), glowIntensity);
	const outsideFactor = smoothstep(-.003, .003, sdf);
	return __tsover_mul(totalGlow, outsideFactor);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "sdf"
			},
			{
				"type": "i",
				"name": "glowRadius"
			},
			{
				"type": "i",
				"name": "glowIntensity"
			}
		],
		"body": [0, [
			[
				13,
				"glowR",
				[
					6,
					"std.max",
					["glowRadius", [5, "0.001"]]
				]
			],
			[
				13,
				"sdfOutside",
				[
					6,
					"std.max",
					["sdf", [5, "0"]]
				]
			],
			[
				13,
				"innerGlow",
				[
					1,
					[
						6,
						"std.exp",
						[[
							1,
							[
								1,
								"sdfOutside",
								"*",
								[
									4,
									"-",
									[5, "1"]
								]
							],
							"*",
							[
								1,
								[5, "12"],
								"/",
								"glowR"
							]
						]]
					],
					"*",
					[5, "0.7"]
				]
			],
			[
				13,
				"outerGlow",
				[
					1,
					[
						6,
						"std.exp",
						[[
							1,
							[
								1,
								"sdfOutside",
								"*",
								[
									4,
									"-",
									[5, "1"]
								]
							],
							"*",
							[
								1,
								[5, "4"],
								"/",
								"glowR"
							]
						]]
					],
					"*",
					[5, "0.35"]
				]
			],
			[
				13,
				"totalGlow",
				[
					1,
					[
						1,
						"innerGlow",
						"+",
						"outerGlow"
					],
					"*",
					"glowIntensity"
				]
			],
			[
				13,
				"outsideFactor",
				[
					6,
					"std.smoothstep",
					[
						[
							4,
							"-",
							[5, "0.003"]
						],
						[5, "0.003"],
						"sdf"
					]
				]
			],
			[10, [
				1,
				"totalGlow",
				"*",
				"outsideFactor"
			]]
		]]
	},
	externals: {
		"std.max": () => max,
		"std.exp": () => exp,
		"std.smoothstep": () => smoothstep
	}
}) && $.f)({}));
const neonFlicker = typegpu_default.fn([
	f32,
	f32,
	f32
], f32)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((time, flickerSpeed, flickerAmount) => {
	"use gpu";
	const flickerTime = __tsover_mul(time, flickerSpeed);
	const f1 = sin(__tsover_mul(flickerTime, 17.13));
	const f2 = sin(__tsover_mul(flickerTime, 31.71));
	const f3 = sin(__tsover_mul(flickerTime, 47.29));
	const combined = __tsover_add(__tsover_add(f1, __tsover_mul(f2, .5)), __tsover_mul(f3, .3));
	const threshold = mix(-2, .8, flickerAmount);
	return smoothstep(__tsover_sub(threshold, .1), __tsover_add(threshold, .1), combined);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "time"
			},
			{
				"type": "i",
				"name": "flickerSpeed"
			},
			{
				"type": "i",
				"name": "flickerAmount"
			}
		],
		"body": [0, [
			[
				13,
				"flickerTime",
				[
					1,
					"time",
					"*",
					"flickerSpeed"
				]
			],
			[
				13,
				"f1",
				[
					6,
					"std.sin",
					[[
						1,
						"flickerTime",
						"*",
						[5, "17.13"]
					]]
				]
			],
			[
				13,
				"f2",
				[
					6,
					"std.sin",
					[[
						1,
						"flickerTime",
						"*",
						[5, "31.71"]
					]]
				]
			],
			[
				13,
				"f3",
				[
					6,
					"std.sin",
					[[
						1,
						"flickerTime",
						"*",
						[5, "47.29"]
					]]
				]
			],
			[
				13,
				"combined",
				[
					1,
					[
						1,
						"f1",
						"+",
						[
							1,
							"f2",
							"*",
							[5, "0.5"]
						]
					],
					"+",
					[
						1,
						"f3",
						"*",
						[5, "0.3"]
					]
				]
			],
			[
				13,
				"threshold",
				[
					6,
					"std.mix",
					[
						[
							4,
							"-",
							[5, "2"]
						],
						[5, "0.8"],
						"flickerAmount"
					]
				]
			],
			[10, [
				6,
				"std.smoothstep",
				[
					[
						1,
						"threshold",
						"-",
						[5, "0.1"]
					],
					[
						1,
						"threshold",
						"+",
						[5, "0.1"]
					],
					"combined"
				]
			]]
		]]
	},
	externals: {
		"std.sin": () => sin,
		"std.mix": () => mix,
		"std.smoothstep": () => smoothstep
	}
}) && $.f)({}));
const neonFlow = typegpu_default.fn([
	vec2f,
	f32,
	f32,
	f32
], f32)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((sdfUV, time, flowSpeed, flowAmount) => {
	"use gpu";
	const flowDx = __tsover_sub(sdfUV.x, .5);
	const flowAngle = atan2(__tsover_sub(sdfUV.y, .5), flowDx);
	const flowPhase = __tsover_add(__tsover_mul(flowAngle, 2), __tsover_mul(time, flowSpeed));
	return mix(1, __tsover_add(__tsover_mul(sin(flowPhase), .5), .5), flowAmount);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "sdfUV"
			},
			{
				"type": "i",
				"name": "time"
			},
			{
				"type": "i",
				"name": "flowSpeed"
			},
			{
				"type": "i",
				"name": "flowAmount"
			}
		],
		"body": [0, [
			[
				13,
				"flowDx",
				[
					1,
					[
						7,
						"sdfUV",
						"x"
					],
					"-",
					[5, "0.5"]
				]
			],
			[
				13,
				"flowDy",
				[
					1,
					[
						7,
						"sdfUV",
						"y"
					],
					"-",
					[5, "0.5"]
				]
			],
			[
				13,
				"flowAngle",
				[
					6,
					"std.atan2",
					["flowDy", "flowDx"]
				]
			],
			[
				13,
				"flowPhase",
				[
					1,
					[
						1,
						"flowAngle",
						"*",
						[5, "2"]
					],
					"+",
					[
						1,
						"time",
						"*",
						"flowSpeed"
					]
				]
			],
			[
				13,
				"flowWave",
				[
					1,
					[
						1,
						[
							6,
							"std.sin",
							["flowPhase"]
						],
						"*",
						[5, "0.5"]
					],
					"+",
					[5, "0.5"]
				]
			],
			[10, [
				6,
				"std.mix",
				[
					[5, "1"],
					"flowWave",
					"flowAmount"
				]
			]]
		]]
	},
	externals: {
		"std.atan2": () => atan2,
		"std.sin": () => sin,
		"std.mix": () => mix
	}
}) && $.f)({}));
const neonComposite = typegpu_default.fn([
	vec4f,
	vec4f,
	vec4f,
	vec4f,
	vec4f,
	f32,
	vec2f,
	f32,
	vec2f,
	f32,
	f32,
	f32,
	vec3f,
	vec3f,
	f32,
	vec3f,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32
], vec4f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((sC, sR, sL, sU, sD, smoothEPS, sdfUV, scale, viewportSize, time, cornerSmoothing, tubeThickness, color, secondaryColor, secondaryBlend, glowColor, intensity, hotCoreIntensity, glowIntensity, glowRadius, lightAngle, specularIntensity, specularSize, flickerSpeed, flickerAmount, flowSpeed, flowAmount, volumetric) => {
	"use gpu";
	const sdfCenter = sC.x;
	const sdfRight = sR.x;
	const sdfLeft = sL.x;
	const sdfUp = sU.x;
	const sdfDown = sD.x;
	const pxH = __tsover_div(1, viewportSize.y);
	const glowR = max(glowRadius, .001);
	const softPx = max(__tsover_div(pxH, scale), .004);
	const gradX = __tsover_div(__tsover_sub(sdfRight, sdfLeft), __tsover_mul(smoothEPS, 2));
	const gradY = __tsover_div(__tsover_sub(sdfUp, sdfDown), __tsover_mul(smoothEPS, 2));
	const sdf = mix(sdfCenter, __tsover_div(__tsover_add(__tsover_add(__tsover_add(__tsover_add(sdfCenter, sdfRight), sdfLeft), sdfUp), sdfDown), 5), cornerSmoothing);
	const gxRaw = select(gradX, __tsover_sub(sR.w, sL.w), volumetric > .5);
	const gyRaw = select(gradY, __tsover_sub(sU.w, sD.w), volumetric > .5);
	const gradLen = max(sqrt(__tsover_add(__tsover_mul(gxRaw, gxRaw), __tsover_mul(gyRaw, gyRaw))), 1e-4);
	const gnx = __tsover_div(gxRaw, gradLen);
	const gny = __tsover_div(gyRaw, gradLen);
	const depthNorm2D = clamp(__tsover_div(__tsover_mul(sdf, -1), max(tubeThickness, .001)), 0, 1);
	const innerMask2D = smoothstep(__tsover_add(tubeThickness, softPx), __tsover_sub(tubeThickness, softPx), __tsover_mul(sdf, -1));
	const lap = __tsover_add(abs(__tsover_sub(__tsover_add(sR.w, sL.w), __tsover_mul(sC.w, 2))), abs(__tsover_sub(__tsover_add(sU.w, sD.w), __tsover_mul(sC.w, 2))));
	const slopeMag = __tsover_add(abs(gxRaw), abs(gyRaw));
	const crease = __tsover_div(lap, __tsover_add(__tsover_add(lap, __tsover_mul(smoothEPS, .6)), __tsover_mul(slopeMag, .35)));
	const innerMaskVol = smoothstep(.08, .35, crease);
	const depthNorm = select(depthNorm2D, crease, volumetric > .5);
	const innerMask = select(innerMask2D, innerMaskVol, volumetric > .5);
	const outerMask = smoothstep(softPx, __tsover_mul(softPx, -1), sdf);
	const tubeMask = __tsover_mul(outerMask, innerMask);
	const t = depthNorm;
	const cylinderZ = sqrt(max(__tsover_mul(t, __tsover_sub(2, t)), 0));
	const cylinderR = __tsover_sub(1, t);
	const nx = __tsover_mul(gnx, cylinderR);
	const ny = __tsover_mul(gny, cylinderR);
	const nz = cylinderZ;
	const lightRad = __tsover_mul(lightAngle, DEG_TO_RAD);
	const lx = cos(lightRad);
	const ly = sin(lightRad);
	const lxn = __tsover_mul(lx, _INV_LIGHT_LEN);
	const lyn = __tsover_mul(ly, _INV_LIGHT_LEN);
	const lzn = _LZN;
	const diffuse = clamp(__tsover_add(__tsover_add(__tsover_mul(nx, lxn), __tsover_mul(ny, lyn)), __tsover_mul(nz, lzn)), 0, 1);
	const NdotH = __tsover_add(__tsover_add(__tsover_mul(nx, __tsover_mul(lxn, _INV_H_LEN)), __tsover_mul(ny, __tsover_mul(lyn, _INV_H_LEN))), __tsover_mul(nz, __tsover_mul(_HZ, _INV_H_LEN)));
	const shininess = exp2(__tsover_sub(8, __tsover_mul(specularSize, 7)));
	const specular = __tsover_mul(pow(clamp(NdotH, 0, 1), shininess), specularIntensity);
	const flowMod = neonFlow(sdfUV, time, flowSpeed, flowAmount);
	const baseColor = mix(color, mix(secondaryColor, color, diffuse), secondaryBlend);
	const ambient = .12;
	const selfEmission = __tsover_mul(cylinderZ, .35);
	const shadedRgb = baseColor.mul(__tsover_add(__tsover_add(ambient, __tsover_mul(diffuse, .55)), selfEmission));
	const tubeRgb = vec3f(__tsover_add(shadedRgb.x, specular), __tsover_add(shadedRgb.y, specular), __tsover_add(shadedRgb.z, specular));
	const coreProfile = smoothstep(.55, 1, depthNorm);
	const hotCoreAmt = __tsover_mul(__tsover_mul(coreProfile, hotCoreIntensity), flowMod);
	const tubeFlowed = mix(tubeRgb, mix(color, vec3f(1, 1, 1), .75), hotCoreAmt).mul(mix(1, flowMod, .4));
	const glowMasked = neonGlow(sdf, glowRadius, glowIntensity);
	const flickerValue = neonFlicker(time, flickerSpeed, flickerAmount);
	const intensityMul = __tsover_mul(intensity, flickerValue);
	const finalTubeRgb = tubeFlowed.mul(intensityMul);
	const finalGlowRgb = glowColor.mul(__tsover_mul(glowMasked, intensityMul));
	const result = vec4f(mix(finalGlowRgb.x, finalTubeRgb.x, tubeMask), mix(finalGlowRgb.y, finalTubeRgb.y, tubeMask), mix(finalGlowRgb.z, finalTubeRgb.z, tubeMask), max(tubeMask, clamp(__tsover_mul(glowMasked, .6), 0, 1)));
	const threshOut = __tsover_add(__tsover_add(__tsover_mul(glowR, 1.5), softPx), smoothEPS);
	return select(result, vec4f(0, 0, 0, 0), sdfCenter > threshOut);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "sC"
			},
			{
				"type": "i",
				"name": "sR"
			},
			{
				"type": "i",
				"name": "sL"
			},
			{
				"type": "i",
				"name": "sU"
			},
			{
				"type": "i",
				"name": "sD"
			},
			{
				"type": "i",
				"name": "smoothEPS"
			},
			{
				"type": "i",
				"name": "sdfUV"
			},
			{
				"type": "i",
				"name": "scale"
			},
			{
				"type": "i",
				"name": "viewportSize"
			},
			{
				"type": "i",
				"name": "time"
			},
			{
				"type": "i",
				"name": "cornerSmoothing"
			},
			{
				"type": "i",
				"name": "tubeThickness"
			},
			{
				"type": "i",
				"name": "color"
			},
			{
				"type": "i",
				"name": "secondaryColor"
			},
			{
				"type": "i",
				"name": "secondaryBlend"
			},
			{
				"type": "i",
				"name": "glowColor"
			},
			{
				"type": "i",
				"name": "intensity"
			},
			{
				"type": "i",
				"name": "hotCoreIntensity"
			},
			{
				"type": "i",
				"name": "glowIntensity"
			},
			{
				"type": "i",
				"name": "glowRadius"
			},
			{
				"type": "i",
				"name": "lightAngle"
			},
			{
				"type": "i",
				"name": "specularIntensity"
			},
			{
				"type": "i",
				"name": "specularSize"
			},
			{
				"type": "i",
				"name": "flickerSpeed"
			},
			{
				"type": "i",
				"name": "flickerAmount"
			},
			{
				"type": "i",
				"name": "flowSpeed"
			},
			{
				"type": "i",
				"name": "flowAmount"
			},
			{
				"type": "i",
				"name": "volumetric"
			}
		],
		"body": [0, [
			[
				13,
				"sdfCenter",
				[
					7,
					"sC",
					"x"
				]
			],
			[
				13,
				"sdfRight",
				[
					7,
					"sR",
					"x"
				]
			],
			[
				13,
				"sdfLeft",
				[
					7,
					"sL",
					"x"
				]
			],
			[
				13,
				"sdfUp",
				[
					7,
					"sU",
					"x"
				]
			],
			[
				13,
				"sdfDown",
				[
					7,
					"sD",
					"x"
				]
			],
			[
				13,
				"pxH",
				[
					1,
					[5, "1"],
					"/",
					[
						7,
						"viewportSize",
						"y"
					]
				]
			],
			[
				13,
				"glowR",
				[
					6,
					"std.max",
					["glowRadius", [5, "0.001"]]
				]
			],
			[
				13,
				"softPx",
				[
					6,
					"std.max",
					[[
						1,
						"pxH",
						"/",
						"scale"
					], [5, "0.004"]]
				]
			],
			[
				13,
				"gradX",
				[
					1,
					[
						1,
						"sdfRight",
						"-",
						"sdfLeft"
					],
					"/",
					[
						1,
						"smoothEPS",
						"*",
						[5, "2"]
					]
				]
			],
			[
				13,
				"gradY",
				[
					1,
					[
						1,
						"sdfUp",
						"-",
						"sdfDown"
					],
					"/",
					[
						1,
						"smoothEPS",
						"*",
						[5, "2"]
					]
				]
			],
			[
				13,
				"smoothedSdf",
				[
					1,
					[
						1,
						[
							1,
							[
								1,
								[
									1,
									"sdfCenter",
									"+",
									"sdfRight"
								],
								"+",
								"sdfLeft"
							],
							"+",
							"sdfUp"
						],
						"+",
						"sdfDown"
					],
					"/",
					[5, "5"]
				]
			],
			[
				13,
				"sdf",
				[
					6,
					"std.mix",
					[
						"sdfCenter",
						"smoothedSdf",
						"cornerSmoothing"
					]
				]
			],
			[
				13,
				"gxRaw",
				[
					6,
					"std.select",
					[
						"gradX",
						[
							1,
							[
								7,
								"sR",
								"w"
							],
							"-",
							[
								7,
								"sL",
								"w"
							]
						],
						[
							1,
							"volumetric",
							">",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"gyRaw",
				[
					6,
					"std.select",
					[
						"gradY",
						[
							1,
							[
								7,
								"sU",
								"w"
							],
							"-",
							[
								7,
								"sD",
								"w"
							]
						],
						[
							1,
							"volumetric",
							">",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"gradLen",
				[
					6,
					"std.max",
					[[
						6,
						"std.sqrt",
						[[
							1,
							[
								1,
								"gxRaw",
								"*",
								"gxRaw"
							],
							"+",
							[
								1,
								"gyRaw",
								"*",
								"gyRaw"
							]
						]]
					], [5, "0.0001"]]
				]
			],
			[
				13,
				"gnx",
				[
					1,
					"gxRaw",
					"/",
					"gradLen"
				]
			],
			[
				13,
				"gny",
				[
					1,
					"gyRaw",
					"/",
					"gradLen"
				]
			],
			[
				13,
				"depthNorm2D",
				[
					6,
					"std.clamp",
					[
						[
							1,
							[
								1,
								"sdf",
								"*",
								[
									4,
									"-",
									[5, "1"]
								]
							],
							"/",
							[
								6,
								"std.max",
								["tubeThickness", [5, "0.001"]]
							]
						],
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"innerMask2D",
				[
					6,
					"std.smoothstep",
					[
						[
							1,
							"tubeThickness",
							"+",
							"softPx"
						],
						[
							1,
							"tubeThickness",
							"-",
							"softPx"
						],
						[
							1,
							"sdf",
							"*",
							[
								4,
								"-",
								[5, "1"]
							]
						]
					]
				]
			],
			[
				13,
				"lap",
				[
					1,
					[
						6,
						"std.abs",
						[[
							1,
							[
								1,
								[
									7,
									"sR",
									"w"
								],
								"+",
								[
									7,
									"sL",
									"w"
								]
							],
							"-",
							[
								1,
								[
									7,
									"sC",
									"w"
								],
								"*",
								[5, "2"]
							]
						]]
					],
					"+",
					[
						6,
						"std.abs",
						[[
							1,
							[
								1,
								[
									7,
									"sU",
									"w"
								],
								"+",
								[
									7,
									"sD",
									"w"
								]
							],
							"-",
							[
								1,
								[
									7,
									"sC",
									"w"
								],
								"*",
								[5, "2"]
							]
						]]
					]
				]
			],
			[
				13,
				"slopeMag",
				[
					1,
					[
						6,
						"std.abs",
						["gxRaw"]
					],
					"+",
					[
						6,
						"std.abs",
						["gyRaw"]
					]
				]
			],
			[
				13,
				"crease",
				[
					1,
					"lap",
					"/",
					[
						1,
						[
							1,
							"lap",
							"+",
							[
								1,
								"smoothEPS",
								"*",
								[5, "0.6"]
							]
						],
						"+",
						[
							1,
							"slopeMag",
							"*",
							[5, "0.35"]
						]
					]
				]
			],
			[
				13,
				"innerMaskVol",
				[
					6,
					"std.smoothstep",
					[
						[5, "0.08"],
						[5, "0.35"],
						"crease"
					]
				]
			],
			[
				13,
				"depthNorm",
				[
					6,
					"std.select",
					[
						"depthNorm2D",
						"crease",
						[
							1,
							"volumetric",
							">",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"innerMask",
				[
					6,
					"std.select",
					[
						"innerMask2D",
						"innerMaskVol",
						[
							1,
							"volumetric",
							">",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"outerMask",
				[
					6,
					"std.smoothstep",
					[
						"softPx",
						[
							1,
							"softPx",
							"*",
							[
								4,
								"-",
								[5, "1"]
							]
						],
						"sdf"
					]
				]
			],
			[
				13,
				"tubeMask",
				[
					1,
					"outerMask",
					"*",
					"innerMask"
				]
			],
			[
				13,
				"t",
				"depthNorm"
			],
			[
				13,
				"cylinderZ",
				[
					6,
					"std.sqrt",
					[[
						6,
						"std.max",
						[[
							1,
							"t",
							"*",
							[
								1,
								[5, "2"],
								"-",
								"t"
							]
						], [5, "0"]]
					]]
				]
			],
			[
				13,
				"cylinderR",
				[
					1,
					[5, "1"],
					"-",
					"t"
				]
			],
			[
				13,
				"nx",
				[
					1,
					"gnx",
					"*",
					"cylinderR"
				]
			],
			[
				13,
				"ny",
				[
					1,
					"gny",
					"*",
					"cylinderR"
				]
			],
			[
				13,
				"nz",
				"cylinderZ"
			],
			[
				13,
				"lightRad",
				[
					1,
					"lightAngle",
					"*",
					"DEG_TO_RAD"
				]
			],
			[
				13,
				"lx",
				[
					6,
					"std.cos",
					["lightRad"]
				]
			],
			[
				13,
				"ly",
				[
					6,
					"std.sin",
					["lightRad"]
				]
			],
			[
				13,
				"lxn",
				[
					1,
					"lx",
					"*",
					"_INV_LIGHT_LEN"
				]
			],
			[
				13,
				"lyn",
				[
					1,
					"ly",
					"*",
					"_INV_LIGHT_LEN"
				]
			],
			[
				13,
				"lzn",
				"_LZN"
			],
			[
				13,
				"NdotL",
				[
					1,
					[
						1,
						[
							1,
							"nx",
							"*",
							"lxn"
						],
						"+",
						[
							1,
							"ny",
							"*",
							"lyn"
						]
					],
					"+",
					[
						1,
						"nz",
						"*",
						"lzn"
					]
				]
			],
			[
				13,
				"diffuse",
				[
					6,
					"std.clamp",
					[
						"NdotL",
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"NdotH",
				[
					1,
					[
						1,
						[
							1,
							"nx",
							"*",
							[
								1,
								"lxn",
								"*",
								"_INV_H_LEN"
							]
						],
						"+",
						[
							1,
							"ny",
							"*",
							[
								1,
								"lyn",
								"*",
								"_INV_H_LEN"
							]
						]
					],
					"+",
					[
						1,
						"nz",
						"*",
						[
							1,
							"_HZ",
							"*",
							"_INV_H_LEN"
						]
					]
				]
			],
			[
				13,
				"shininess",
				[
					6,
					"std.exp2",
					[[
						1,
						[5, "8"],
						"-",
						[
							1,
							"specularSize",
							"*",
							[5, "7"]
						]
					]]
				]
			],
			[
				13,
				"specular",
				[
					1,
					[
						6,
						"std.pow",
						[[
							6,
							"std.clamp",
							[
								"NdotH",
								[5, "0"],
								[5, "1"]
							]
						], "shininess"]
					],
					"*",
					"specularIntensity"
				]
			],
			[
				13,
				"flowMod",
				[
					6,
					"neonFlow",
					[
						"sdfUV",
						"time",
						"flowSpeed",
						"flowAmount"
					]
				]
			],
			[
				13,
				"twoTone",
				[
					6,
					"std.mix",
					[
						"secondaryColor",
						"color",
						"diffuse"
					]
				]
			],
			[
				13,
				"baseColor",
				[
					6,
					"std.mix",
					[
						"color",
						"twoTone",
						"secondaryBlend"
					]
				]
			],
			[
				13,
				"ambient",
				[5, "0.12"]
			],
			[
				13,
				"selfEmission",
				[
					1,
					"cylinderZ",
					"*",
					[5, "0.35"]
				]
			],
			[
				13,
				"shadedRgb",
				[
					6,
					[
						7,
						"baseColor",
						"mul"
					],
					[[
						1,
						[
							1,
							"ambient",
							"+",
							[
								1,
								"diffuse",
								"*",
								[5, "0.55"]
							]
						],
						"+",
						"selfEmission"
					]]
				]
			],
			[
				13,
				"tubeRgb",
				[
					6,
					"d.vec3f",
					[
						[
							1,
							[
								7,
								"shadedRgb",
								"x"
							],
							"+",
							"specular"
						],
						[
							1,
							[
								7,
								"shadedRgb",
								"y"
							],
							"+",
							"specular"
						],
						[
							1,
							[
								7,
								"shadedRgb",
								"z"
							],
							"+",
							"specular"
						]
					]
				]
			],
			[
				13,
				"coreProfile",
				[
					6,
					"std.smoothstep",
					[
						[5, "0.55"],
						[5, "1"],
						"depthNorm"
					]
				]
			],
			[
				13,
				"hotCoreAmt",
				[
					1,
					[
						1,
						"coreProfile",
						"*",
						"hotCoreIntensity"
					],
					"*",
					"flowMod"
				]
			],
			[
				13,
				"hotCoreCol",
				[
					6,
					"std.mix",
					[
						"color",
						[
							6,
							"d.vec3f",
							[
								[5, "1"],
								[5, "1"],
								[5, "1"]
							]
						],
						[5, "0.75"]
					]
				]
			],
			[
				13,
				"tubeWithCore",
				[
					6,
					"std.mix",
					[
						"tubeRgb",
						"hotCoreCol",
						"hotCoreAmt"
					]
				]
			],
			[
				13,
				"tubeFlowed",
				[
					6,
					[
						7,
						"tubeWithCore",
						"mul"
					],
					[[
						6,
						"std.mix",
						[
							[5, "1"],
							"flowMod",
							[5, "0.4"]
						]
					]]
				]
			],
			[
				13,
				"glowMasked",
				[
					6,
					"neonGlow",
					[
						"sdf",
						"glowRadius",
						"glowIntensity"
					]
				]
			],
			[
				13,
				"flickerValue",
				[
					6,
					"neonFlicker",
					[
						"time",
						"flickerSpeed",
						"flickerAmount"
					]
				]
			],
			[
				13,
				"intensityMul",
				[
					1,
					"intensity",
					"*",
					"flickerValue"
				]
			],
			[
				13,
				"finalTubeRgb",
				[
					6,
					[
						7,
						"tubeFlowed",
						"mul"
					],
					["intensityMul"]
				]
			],
			[
				13,
				"finalGlowRgb",
				[
					6,
					[
						7,
						"glowColor",
						"mul"
					],
					[[
						1,
						"glowMasked",
						"*",
						"intensityMul"
					]]
				]
			],
			[
				13,
				"finalR",
				[
					6,
					"std.mix",
					[
						[
							7,
							"finalGlowRgb",
							"x"
						],
						[
							7,
							"finalTubeRgb",
							"x"
						],
						"tubeMask"
					]
				]
			],
			[
				13,
				"finalG",
				[
					6,
					"std.mix",
					[
						[
							7,
							"finalGlowRgb",
							"y"
						],
						[
							7,
							"finalTubeRgb",
							"y"
						],
						"tubeMask"
					]
				]
			],
			[
				13,
				"finalB",
				[
					6,
					"std.mix",
					[
						[
							7,
							"finalGlowRgb",
							"z"
						],
						[
							7,
							"finalTubeRgb",
							"z"
						],
						"tubeMask"
					]
				]
			],
			[
				13,
				"glowAlpha",
				[
					6,
					"std.clamp",
					[
						[
							1,
							"glowMasked",
							"*",
							[5, "0.6"]
						],
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"finalA",
				[
					6,
					"std.max",
					["tubeMask", "glowAlpha"]
				]
			],
			[
				13,
				"result",
				[
					6,
					"d.vec4f",
					[
						"finalR",
						"finalG",
						"finalB",
						"finalA"
					]
				]
			],
			[
				13,
				"threshOut",
				[
					1,
					[
						1,
						[
							1,
							"glowR",
							"*",
							[5, "1.5"]
						],
						"+",
						"softPx"
					],
					"+",
					"smoothEPS"
				]
			],
			[10, [
				6,
				"std.select",
				[
					"result",
					[
						6,
						"d.vec4f",
						[
							[5, "0"],
							[5, "0"],
							[5, "0"],
							[5, "0"]
						]
					],
					[
						1,
						"sdfCenter",
						">",
						"threshOut"
					]
				]
			]]
		]]
	},
	externals: {
		"std.max": () => max,
		"std.mix": () => mix,
		"std.select": () => select,
		"std.sqrt": () => sqrt,
		"std.clamp": () => clamp,
		"std.smoothstep": () => smoothstep,
		"std.abs": () => abs,
		"DEG_TO_RAD": () => DEG_TO_RAD,
		"std.cos": () => cos,
		"std.sin": () => sin,
		"_INV_LIGHT_LEN": () => _INV_LIGHT_LEN,
		"_LZN": () => _LZN,
		"_INV_H_LEN": () => _INV_H_LEN,
		"_HZ": () => _HZ,
		"std.exp2": () => exp2,
		"std.pow": () => pow,
		"neonFlow": () => neonFlow,
		"d.vec3f": () => vec3f,
		"neonGlow": () => neonGlow,
		"neonFlicker": () => neonFlicker,
		"d.vec4f": () => vec4f
	}
}) && $.f)({}));
function applyNeonEffect$1(params, sdfSampler, time, options) {
	const { uniforms, ctx } = params;
	const gradSampler = options?.gradSampler ?? sdfSampler;
	const volFlag = floatE(options?.volumetric ? 1 : 0);
	const sdfUV = call(sdfSpaceUV, "sdfSpaceUV", [
		uniforms.center,
		uniforms.scale,
		uniforms.rotation,
		ctx.uv,
		ctx.aspect
	]);
	const smoothEPS = call(neonSmoothEPS, "neonSmoothEPS", [
		uniforms.tubeThickness,
		uniforms.cornerSmoothing,
		volFlag
	]);
	const negEPS = smoothEPS.mul(-1);
	return call(neonComposite, "neonComposite", [
		sdfSampler(sdfUV),
		gradSampler(call(offsetUV, "offsetUV", [
			sdfUV,
			smoothEPS,
			floatE(0)
		])),
		gradSampler(call(offsetUV, "offsetUV", [
			sdfUV,
			negEPS,
			floatE(0)
		])),
		gradSampler(call(offsetUV, "offsetUV", [
			sdfUV,
			floatE(0),
			smoothEPS
		])),
		gradSampler(call(offsetUV, "offsetUV", [
			sdfUV,
			floatE(0),
			negEPS
		])),
		smoothEPS,
		sdfUV,
		uniforms.scale,
		ctx.viewportSize,
		time,
		uniforms.cornerSmoothing,
		uniforms.tubeThickness,
		uniforms.color.member("rgb"),
		uniforms.secondaryColor.member("rgb"),
		uniforms.secondaryBlend,
		uniforms.glowColor.member("rgb"),
		uniforms.intensity,
		uniforms.hotCoreIntensity,
		uniforms.glowIntensity,
		uniforms.glowRadius,
		uniforms.lightAngle,
		uniforms.specularIntensity,
		uniforms.specularSize,
		uniforms.flickerSpeed,
		uniforms.flickerAmount,
		uniforms.flowSpeed,
		uniforms.flowAmount,
		volFlag
	]);
}
var { applyNeonEffect } = neon_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
const componentDefinition = defineStd({
	name: "Neon",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Photorealistic neon tube / 3D pipe effect driven by a custom shape",
	...shapedSurface({
		chord: "firstLobe",
		stencil: "none",
		surface: (frame, params) => applyNeonEffect(params, frame.sampler, params.ctx.time, {
			volumetric: frame.volumetric,
			gradSampler: frame.gradSampler
		})
	}),
	props: {
		origin: {
			default: "center",
			description: "Reference edge the center position is measured from (center default). Lets you pin the shape relative to a corner or the canvas centre.",
			ui: {
				type: "origin",
				label: "Origin",
				group: "Position"
			}
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the neon shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the neon shape (1 = default size)",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .01,
				label: "Scale",
				group: "Position"
			}
		},
		rotation: {
			default: 0,
			description: "Rotation of the neon shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		color: {
			default: "#00ddff",
			transform: transformColor,
			description: "Primary neon tube color",
			ui: {
				type: "color",
				label: "Tube Color",
				group: "Colors"
			}
		},
		secondaryColor: {
			default: "#ff00aa",
			transform: transformColor,
			description: "Shadow-side color for a two-tone / dual-lit pipe look",
			ui: {
				type: "color",
				label: "Secondary Color",
				group: "Colors"
			}
		},
		secondaryBlend: {
			default: .5,
			description: "Blend between mono (0) and two-tone (1) tube coloring",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Blend",
				group: "Colors"
			}
		},
		glowColor: {
			default: "#00ddff",
			transform: transformColor,
			description: "Color of the outer glow / bloom",
			ui: {
				type: "color",
				label: "Glow Color",
				group: "Colors"
			}
		},
		tubeThickness: {
			default: .2,
			transform: (v) => v * .05,
			description: "How far inward from the boundary the tube extends. Low = thin neon outline, high = thick 3D pipe",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Tube Thickness",
				group: "Tube"
			}
		},
		intensity: {
			default: 1.5,
			description: "Overall brightness multiplier",
			ui: {
				type: "range",
				min: .5,
				max: 4,
				step: .01,
				label: "Intensity",
				group: "Tube"
			}
		},
		hotCoreIntensity: {
			default: .6,
			description: "Bright white-hot center line — the gas discharge glow inside the tube",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Hot Core",
				group: "Tube"
			}
		},
		glowIntensity: {
			default: .6,
			description: "Outer glow / bloom strength",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Glow Intensity",
				group: "Glow"
			}
		},
		glowRadius: {
			default: .25,
			description: "How far the glow extends beyond the tube",
			ui: {
				type: "range",
				min: .01,
				max: 1,
				step: .01,
				label: "Glow Radius",
				group: "Glow"
			}
		},
		lightAngle: {
			default: 300,
			description: "Directional light angle in degrees — controls 3D shading on the tube",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Lighting"
			}
		},
		specularIntensity: {
			default: .5,
			description: "Specular highlight brightness on the tube surface",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Specular",
				group: "Lighting"
			}
		},
		specularSize: {
			default: .5,
			description: "Specular highlight size — 0 = tight pinpoint, 1 = broad sheen",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Specular Size",
				group: "Lighting"
			}
		},
		cornerSmoothing: {
			default: .15,
			description: "Rounds sharp corners to mimic how real glass tubes curve at bends",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Corner Smoothing",
				group: "Tube"
			}
		},
		flickerSpeed: {
			default: 0,
			description: "Flicker animation speed — 0 = off, higher = faster sporadic on/off",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Flicker"
			}
		},
		flickerAmount: {
			default: .2,
			description: "How often the neon flickers off — 0 = always on, 1 = frequent outages",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Amount",
				group: "Flicker"
			}
		},
		flowSpeed: {
			default: 0,
			description: "Flow animation speed — 0 = off, light rotates through the tube",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Flow"
			}
		},
		flowAmount: {
			default: .3,
			description: "Strength of the flowing brightness variation — 0 = uniform, 1 = dramatic",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Amount",
				group: "Flow"
			}
		},
		shape: {
			default: DEFAULT_SHAPE_CONFIG,
			description: "Serialized shape configuration (JSON)",
			ui: {
				type: "shape",
				label: "Shape",
				group: "Shape"
			}
		},
		shapeSdfUrl: {
			default: "",
			compileTime: true,
			description: "URL to a pre-generated SDF .bin file — when non-empty, activates SVG mode and triggers a shader recompile"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type — triggers recompile when shape is switched. When empty, derived from shape JSON at mount time."
		}
	}
});
var Neon_default = componentDefinition;
export { componentDefinition as n, Neon_default as t };
