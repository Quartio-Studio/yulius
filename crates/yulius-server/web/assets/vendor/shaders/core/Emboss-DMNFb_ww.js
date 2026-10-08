import { $f as f32, Ac as defineStd, Bf as sqrt, Cf as select, Df as cos, Ef as clamp, Hd as call, Hf as vec2f, Kd as floatE, Nf as max, Rf as sin, Ru as shapeEffectBoundingBoxDeclaration, Uf as vec3f, Wf as vec4f, bu as offsetUV, hi as shapedSurface, if as unpremultiplyAlpha, np as __export, wf as abs, xf as typegpu_default, xu as sdfSpaceUV, zf as smoothstep } from "./std-ClftfWpF.js";
import { c as transformPosition } from "./transformations-P50Uo_Ju.js";
var emboss_exports = /* @__PURE__ */ __export({
	applyEmbossEffect: () => applyEmbossEffect$1,
	embossComposite: () => embossComposite,
	embossEdgeLighting: () => embossEdgeLighting,
	embossHeight: () => embossHeight,
	embossTraceUV: () => embossTraceUV,
	embossWarpUV: () => embossWarpUV
});
var DEG_TO_RAD = .017453292519943295;
var _INV_LIGHT_LEN = 1 / Math.sqrt(1.49);
var _LZN = .7 * _INV_LIGHT_LEN;
const embossHeight = typegpu_default.fn([
	f32,
	f32,
	f32,
	f32
], f32)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((s, depth, transitionW, volumetric) => {
	"use gpu";
	const vol = __tsover_mul(depth, clamp(__tsover_div(__tsover_mul(s, -1), .18), 0, 1));
	return select(__tsover_mul(depth, smoothstep(transitionW, __tsover_mul(transitionW, -1), s)), vol, volumetric > .5);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "s"
			},
			{
				"type": "i",
				"name": "depth"
			},
			{
				"type": "i",
				"name": "transitionW"
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
					1,
					"depth",
					"*",
					[
						6,
						"std.clamp",
						[
							[
								1,
								[
									1,
									"s",
									"*",
									[
										4,
										"-",
										[5, "1"]
									]
								],
								"/",
								[5, "0.18"]
							],
							[5, "0"],
							[5, "1"]
						]
					]
				]
			],
			[
				13,
				"flat",
				[
					1,
					"depth",
					"*",
					[
						6,
						"std.smoothstep",
						[
							"transitionW",
							[
								1,
								"transitionW",
								"*",
								[
									4,
									"-",
									[5, "1"]
								]
							],
							"s"
						]
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
		"std.clamp": () => clamp,
		"std.smoothstep": () => smoothstep,
		"std.select": () => select
	}
}) && $.f)({}));
const embossTraceUV = typegpu_default.fn([
	vec2f,
	f32,
	f32,
	f32
], vec2f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((sdfUV, lightAngle, depth, frac) => {
	"use gpu";
	const lx = cos(__tsover_mul(lightAngle, DEG_TO_RAD));
	const ly = sin(__tsover_mul(lightAngle, DEG_TO_RAD));
	const absDepth = max(abs(depth), 1e-4);
	const spread = __tsover_mul(absDepth, .06);
	return vec2f(__tsover_add(sdfUV.x, __tsover_mul(__tsover_mul(lx, spread), frac)), __tsover_add(sdfUV.y, __tsover_mul(__tsover_mul(ly, spread), frac)));
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
				"name": "lightAngle"
			},
			{
				"type": "i",
				"name": "depth"
			},
			{
				"type": "i",
				"name": "frac"
			}
		],
		"body": [0, [
			[
				13,
				"lx",
				[
					6,
					"std.cos",
					[[
						1,
						"lightAngle",
						"*",
						"DEG_TO_RAD"
					]]
				]
			],
			[
				13,
				"ly",
				[
					6,
					"std.sin",
					[[
						1,
						"lightAngle",
						"*",
						"DEG_TO_RAD"
					]]
				]
			],
			[
				13,
				"absDepth",
				[
					6,
					"std.max",
					[[
						6,
						"std.abs",
						["depth"]
					], [5, "0.0001"]]
				]
			],
			[
				13,
				"spread",
				[
					1,
					"absDepth",
					"*",
					[5, "0.06"]
				]
			],
			[10, [
				6,
				"d.vec2f",
				[[
					1,
					[
						7,
						"sdfUV",
						"x"
					],
					"+",
					[
						1,
						[
							1,
							"lx",
							"*",
							"spread"
						],
						"*",
						"frac"
					]
				], [
					1,
					[
						7,
						"sdfUV",
						"y"
					],
					"+",
					[
						1,
						[
							1,
							"ly",
							"*",
							"spread"
						],
						"*",
						"frac"
					]
				]]
			]]
		]]
	},
	externals: {
		"std.cos": () => cos,
		"DEG_TO_RAD": () => DEG_TO_RAD,
		"std.sin": () => sin,
		"std.max": () => max,
		"std.abs": () => abs,
		"d.vec2f": () => vec2f
	}
}) && $.f)({}));
const embossWarpUV = typegpu_default.fn([
	vec2f,
	vec2f,
	f32,
	f32
], vec2f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((uv, center, sdfC, depth) => {
	"use gpu";
	const tw = .003;
	const inside = smoothstep(tw, __tsover_mul(tw, -1), sdfC);
	const dispScale = __tsover_sub(1, __tsover_mul(__tsover_mul(depth, .05), inside));
	const centerX = center.x;
	const centerY = __tsover_sub(1, center.y);
	return vec2f(__tsover_add(centerX, __tsover_mul(__tsover_sub(uv.x, centerX), dispScale)), __tsover_add(centerY, __tsover_mul(__tsover_sub(uv.y, centerY), dispScale)));
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "uv"
			},
			{
				"type": "i",
				"name": "center"
			},
			{
				"type": "i",
				"name": "sdfC"
			},
			{
				"type": "i",
				"name": "depth"
			}
		],
		"body": [0, [
			[
				13,
				"tw",
				[5, "0.003"]
			],
			[
				13,
				"inside",
				[
					6,
					"std.smoothstep",
					[
						"tw",
						[
							1,
							"tw",
							"*",
							[
								4,
								"-",
								[5, "1"]
							]
						],
						"sdfC"
					]
				]
			],
			[
				13,
				"dispScale",
				[
					1,
					[5, "1"],
					"-",
					[
						1,
						[
							1,
							"depth",
							"*",
							[5, "0.05"]
						],
						"*",
						"inside"
					]
				]
			],
			[
				13,
				"centerX",
				[
					7,
					"center",
					"x"
				]
			],
			[
				13,
				"centerY",
				[
					1,
					[5, "1"],
					"-",
					[
						7,
						"center",
						"y"
					]
				]
			],
			[10, [
				6,
				"d.vec2f",
				[[
					1,
					"centerX",
					"+",
					[
						1,
						[
							1,
							[
								7,
								"uv",
								"x"
							],
							"-",
							"centerX"
						],
						"*",
						"dispScale"
					]
				], [
					1,
					"centerY",
					"+",
					[
						1,
						[
							1,
							[
								7,
								"uv",
								"y"
							],
							"-",
							"centerY"
						],
						"*",
						"dispScale"
					]
				]]
			]]
		]]
	},
	externals: {
		"std.smoothstep": () => smoothstep,
		"d.vec2f": () => vec2f
	}
}) && $.f)({}));
const embossEdgeLighting = typegpu_default.fn([
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32,
	f32
], vec2f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((sdfR, sdfL, sdfU, sdfD, depth, lightAngle, lightIntensity, volumetric) => {
	"use gpu";
	const eps = .003;
	const tw = .003;
	const hR = embossHeight(sdfR, depth, tw, volumetric);
	const hL = embossHeight(sdfL, depth, tw, volumetric);
	const hU = embossHeight(sdfU, depth, tw, volumetric);
	const hD = embossHeight(sdfD, depth, tw, volumetric);
	const dhx = __tsover_div(__tsover_sub(hR, hL), __tsover_mul(eps, 2));
	const dhy = __tsover_div(__tsover_sub(hU, hD), __tsover_mul(eps, 2));
	const nLen = sqrt(__tsover_add(__tsover_add(__tsover_mul(dhx, dhx), __tsover_mul(dhy, dhy)), 1));
	const nx = __tsover_div(__tsover_mul(dhx, -1), nLen);
	const ny = __tsover_div(__tsover_mul(dhy, -1), nLen);
	const nz = __tsover_div(1, nLen);
	const lx = cos(__tsover_mul(lightAngle, DEG_TO_RAD));
	const ly = sin(__tsover_mul(lightAngle, DEG_TO_RAD));
	const lxn = __tsover_mul(lx, _INV_LIGHT_LEN);
	const lyn = __tsover_mul(ly, _INV_LIGHT_LEN);
	const lzn = _LZN;
	const NdotL = __tsover_add(__tsover_add(__tsover_mul(nx, lxn), __tsover_mul(ny, lyn)), __tsover_mul(nz, lzn));
	const edgeLighting = __tsover_mul(__tsover_sub(NdotL, lzn), lightIntensity);
	return vec2f(max(edgeLighting, 0), max(__tsover_mul(edgeLighting, -1), 0));
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "sdfR"
			},
			{
				"type": "i",
				"name": "sdfL"
			},
			{
				"type": "i",
				"name": "sdfU"
			},
			{
				"type": "i",
				"name": "sdfD"
			},
			{
				"type": "i",
				"name": "depth"
			},
			{
				"type": "i",
				"name": "lightAngle"
			},
			{
				"type": "i",
				"name": "lightIntensity"
			},
			{
				"type": "i",
				"name": "volumetric"
			}
		],
		"body": [0, [
			[
				13,
				"eps",
				[5, "0.003"]
			],
			[
				13,
				"tw",
				[5, "0.003"]
			],
			[
				13,
				"hR",
				[
					6,
					"embossHeight",
					[
						"sdfR",
						"depth",
						"tw",
						"volumetric"
					]
				]
			],
			[
				13,
				"hL",
				[
					6,
					"embossHeight",
					[
						"sdfL",
						"depth",
						"tw",
						"volumetric"
					]
				]
			],
			[
				13,
				"hU",
				[
					6,
					"embossHeight",
					[
						"sdfU",
						"depth",
						"tw",
						"volumetric"
					]
				]
			],
			[
				13,
				"hD",
				[
					6,
					"embossHeight",
					[
						"sdfD",
						"depth",
						"tw",
						"volumetric"
					]
				]
			],
			[
				13,
				"dhx",
				[
					1,
					[
						1,
						"hR",
						"-",
						"hL"
					],
					"/",
					[
						1,
						"eps",
						"*",
						[5, "2"]
					]
				]
			],
			[
				13,
				"dhy",
				[
					1,
					[
						1,
						"hU",
						"-",
						"hD"
					],
					"/",
					[
						1,
						"eps",
						"*",
						[5, "2"]
					]
				]
			],
			[
				13,
				"nLen",
				[
					6,
					"std.sqrt",
					[[
						1,
						[
							1,
							[
								1,
								"dhx",
								"*",
								"dhx"
							],
							"+",
							[
								1,
								"dhy",
								"*",
								"dhy"
							]
						],
						"+",
						[5, "1"]
					]]
				]
			],
			[
				13,
				"nx",
				[
					1,
					[
						1,
						"dhx",
						"*",
						[
							4,
							"-",
							[5, "1"]
						]
					],
					"/",
					"nLen"
				]
			],
			[
				13,
				"ny",
				[
					1,
					[
						1,
						"dhy",
						"*",
						[
							4,
							"-",
							[5, "1"]
						]
					],
					"/",
					"nLen"
				]
			],
			[
				13,
				"nz",
				[
					1,
					[5, "1"],
					"/",
					"nLen"
				]
			],
			[
				13,
				"lx",
				[
					6,
					"std.cos",
					[[
						1,
						"lightAngle",
						"*",
						"DEG_TO_RAD"
					]]
				]
			],
			[
				13,
				"ly",
				[
					6,
					"std.sin",
					[[
						1,
						"lightAngle",
						"*",
						"DEG_TO_RAD"
					]]
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
				"edgeLighting",
				[
					1,
					[
						1,
						"NdotL",
						"-",
						"lzn"
					],
					"*",
					"lightIntensity"
				]
			],
			[
				13,
				"highlight",
				[
					6,
					"std.max",
					["edgeLighting", [5, "0"]]
				]
			],
			[
				13,
				"edgeShadow",
				[
					6,
					"std.max",
					[[
						1,
						"edgeLighting",
						"*",
						[
							4,
							"-",
							[5, "1"]
						]
					], [5, "0"]]
				]
			],
			[10, [
				6,
				"d.vec2f",
				["highlight", "edgeShadow"]
			]]
		]]
	},
	externals: {
		"embossHeight": () => embossHeight,
		"std.sqrt": () => sqrt,
		"std.cos": () => cos,
		"DEG_TO_RAD": () => DEG_TO_RAD,
		"std.sin": () => sin,
		"_INV_LIGHT_LEN": () => _INV_LIGHT_LEN,
		"_LZN": () => _LZN,
		"std.max": () => max,
		"d.vec2f": () => vec2f
	}
}) && $.f)({}));
const embossComposite = typegpu_default.fn([
	vec4f,
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
], vec4f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((childColor, sdfC, sdfR, sdfL, sdfU, sdfD, h1raw, h2raw, depth, lightAngle, lightIntensity, shadowIntensity, volumetric) => {
	"use gpu";
	const tw = .003;
	const light = embossEdgeLighting(sdfR, sdfL, sdfU, sdfD, depth, lightAngle, lightIntensity, volumetric);
	const highlight = light.x;
	const edgeShadow = light.y;
	const hC = embossHeight(sdfC, depth, tw, volumetric);
	const absDepth = max(abs(depth), 1e-4);
	const shadowTw = __tsover_mul(absDepth, .06);
	const h1 = embossHeight(h1raw, depth, shadowTw, volumetric);
	const h2 = embossHeight(h2raw, depth, shadowTw, volumetric);
	const diff1 = __tsover_div(__tsover_sub(h1, hC), absDepth);
	const diff2 = __tsover_div(__tsover_sub(h2, hC), absDepth);
	const castShadow = __tsover_add(__tsover_mul(smoothstep(0, .5, diff1), .55), __tsover_mul(smoothstep(0, .5, diff2), .45));
	const totalShadow = __tsover_mul(clamp(__tsover_add(edgeShadow, castShadow), 0, 1), shadowIntensity);
	const childRgb = vec3f(childColor.x, childColor.y, childColor.z);
	const highlightRgb = vec3f(highlight, highlight, highlight);
	const finalRgb = clamp(childRgb.mul(__tsover_sub(1, totalShadow)).add(highlightRgb), vec3f(0), vec3f(1));
	return vec4f(finalRgb.x, finalRgb.y, finalRgb.z, childColor.w);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "childColor"
			},
			{
				"type": "i",
				"name": "sdfC"
			},
			{
				"type": "i",
				"name": "sdfR"
			},
			{
				"type": "i",
				"name": "sdfL"
			},
			{
				"type": "i",
				"name": "sdfU"
			},
			{
				"type": "i",
				"name": "sdfD"
			},
			{
				"type": "i",
				"name": "h1raw"
			},
			{
				"type": "i",
				"name": "h2raw"
			},
			{
				"type": "i",
				"name": "depth"
			},
			{
				"type": "i",
				"name": "lightAngle"
			},
			{
				"type": "i",
				"name": "lightIntensity"
			},
			{
				"type": "i",
				"name": "shadowIntensity"
			},
			{
				"type": "i",
				"name": "volumetric"
			}
		],
		"body": [0, [
			[
				13,
				"tw",
				[5, "0.003"]
			],
			[
				13,
				"light",
				[
					6,
					"embossEdgeLighting",
					[
						"sdfR",
						"sdfL",
						"sdfU",
						"sdfD",
						"depth",
						"lightAngle",
						"lightIntensity",
						"volumetric"
					]
				]
			],
			[
				13,
				"highlight",
				[
					7,
					"light",
					"x"
				]
			],
			[
				13,
				"edgeShadow",
				[
					7,
					"light",
					"y"
				]
			],
			[
				13,
				"hC",
				[
					6,
					"embossHeight",
					[
						"sdfC",
						"depth",
						"tw",
						"volumetric"
					]
				]
			],
			[
				13,
				"absDepth",
				[
					6,
					"std.max",
					[[
						6,
						"std.abs",
						["depth"]
					], [5, "0.0001"]]
				]
			],
			[
				13,
				"shadowTw",
				[
					1,
					"absDepth",
					"*",
					[5, "0.06"]
				]
			],
			[
				13,
				"h1",
				[
					6,
					"embossHeight",
					[
						"h1raw",
						"depth",
						"shadowTw",
						"volumetric"
					]
				]
			],
			[
				13,
				"h2",
				[
					6,
					"embossHeight",
					[
						"h2raw",
						"depth",
						"shadowTw",
						"volumetric"
					]
				]
			],
			[
				13,
				"diff1",
				[
					1,
					[
						1,
						"h1",
						"-",
						"hC"
					],
					"/",
					"absDepth"
				]
			],
			[
				13,
				"diff2",
				[
					1,
					[
						1,
						"h2",
						"-",
						"hC"
					],
					"/",
					"absDepth"
				]
			],
			[
				13,
				"castShadow",
				[
					1,
					[
						1,
						[
							6,
							"std.smoothstep",
							[
								[5, "0"],
								[5, "0.5"],
								"diff1"
							]
						],
						"*",
						[5, "0.55"]
					],
					"+",
					[
						1,
						[
							6,
							"std.smoothstep",
							[
								[5, "0"],
								[5, "0.5"],
								"diff2"
							]
						],
						"*",
						[5, "0.45"]
					]
				]
			],
			[
				13,
				"totalShadow",
				[
					1,
					[
						6,
						"std.clamp",
						[
							[
								1,
								"edgeShadow",
								"+",
								"castShadow"
							],
							[5, "0"],
							[5, "1"]
						]
					],
					"*",
					"shadowIntensity"
				]
			],
			[
				13,
				"childRgb",
				[
					6,
					"d.vec3f",
					[
						[
							7,
							"childColor",
							"x"
						],
						[
							7,
							"childColor",
							"y"
						],
						[
							7,
							"childColor",
							"z"
						]
					]
				]
			],
			[
				13,
				"highlightRgb",
				[
					6,
					"d.vec3f",
					[
						"highlight",
						"highlight",
						"highlight"
					]
				]
			],
			[
				13,
				"finalRgb",
				[
					6,
					"std.clamp",
					[
						[
							6,
							[
								7,
								[
									6,
									[
										7,
										"childRgb",
										"mul"
									],
									[[
										1,
										[5, "1"],
										"-",
										"totalShadow"
									]]
								],
								"add"
							],
							["highlightRgb"]
						],
						[
							6,
							"d.vec3f",
							[[5, "0"]]
						],
						[
							6,
							"d.vec3f",
							[[5, "1"]]
						]
					]
				]
			],
			[10, [
				6,
				"d.vec4f",
				[
					[
						7,
						"finalRgb",
						"x"
					],
					[
						7,
						"finalRgb",
						"y"
					],
					[
						7,
						"finalRgb",
						"z"
					],
					[
						7,
						"childColor",
						"w"
					]
				]
			]]
		]]
	},
	externals: {
		"embossEdgeLighting": () => embossEdgeLighting,
		"embossHeight": () => embossHeight,
		"std.max": () => max,
		"std.abs": () => abs,
		"std.smoothstep": () => smoothstep,
		"std.clamp": () => clamp,
		"d.vec3f": () => vec3f,
		"d.vec4f": () => vec4f
	}
}) && $.f)({}));
function applyEmbossEffect$1(params, sdfSampler, childTexture, options) {
	const { uniforms, ctx } = params;
	const gradSampler = options?.gradSampler ?? sdfSampler;
	const volFlag = floatE(options?.volumetric ? 1 : 0);
	const eps = .003;
	const sdfUV = call(sdfSpaceUV, "sdfSpaceUV", [
		uniforms.center,
		uniforms.scale,
		uniforms.rotation,
		ctx.uv,
		ctx.aspect
	]);
	const sdfC = sdfSampler(sdfUV).member("r");
	const sdfR = gradSampler(call(offsetUV, "offsetUV", [
		sdfUV,
		floatE(eps),
		floatE(0)
	])).member("r");
	const sdfL = gradSampler(call(offsetUV, "offsetUV", [
		sdfUV,
		floatE(-eps),
		floatE(0)
	])).member("r");
	const sdfU = gradSampler(call(offsetUV, "offsetUV", [
		sdfUV,
		floatE(0),
		floatE(eps)
	])).member("r");
	const sdfD = gradSampler(call(offsetUV, "offsetUV", [
		sdfUV,
		floatE(0),
		floatE(-eps)
	])).member("r");
	const h1raw = gradSampler(call(embossTraceUV, "embossTraceUV", [
		sdfUV,
		uniforms.lightAngle,
		uniforms.depth,
		floatE(.5)
	])).member("r");
	const h2raw = gradSampler(call(embossTraceUV, "embossTraceUV", [
		sdfUV,
		uniforms.lightAngle,
		uniforms.depth,
		floatE(1)
	])).member("r");
	const warpedUV = call(embossWarpUV, "embossWarpUV", [
		ctx.uv,
		uniforms.center,
		sdfC,
		uniforms.depth
	]);
	return call(embossComposite, "embossComposite", [
		call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(warpedUV)]),
		sdfC,
		sdfR,
		sdfL,
		sdfU,
		sdfD,
		h1raw,
		h2raw,
		uniforms.depth,
		uniforms.lightAngle,
		uniforms.lightIntensity,
		uniforms.shadowIntensity,
		volFlag
	]);
}
var { applyEmbossEffect } = emboss_exports;
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
const componentDefinition = defineStd({
	name: "Emboss",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Embossed / debossed relief shading on top of child content, driven by a custom shape",
	requiresRTT: true,
	requiresChild: true,
	...shapedSurface({
		chord: "firstLobe",
		stencil: "none",
		child: "required",
		surface: (frame, params) => applyEmbossEffect(params, frame.sampler, frame.childTexture, {
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
			description: "Center position of the embossed shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the embossed shape (1 = default size)",
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
			description: "Rotation of the embossed shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		depth: {
			default: -.5,
			description: "Relief depth — negative = inset (debossed), positive = raised (embossed)",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Depth",
				group: "Relief"
			}
		},
		lightAngle: {
			default: 260,
			description: "Directional light angle in degrees — controls highlight and shadow direction",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Lighting"
			}
		},
		lightIntensity: {
			default: .6,
			description: "Strength of the directional edge highlights and shadows",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Light Intensity",
				group: "Lighting"
			}
		},
		shadowIntensity: {
			default: .3,
			description: "Darkness of the relief shadow",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Shadow Intensity",
				group: "Relief"
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
var Emboss_default = componentDefinition;
export { componentDefinition as n, Emboss_default as t };
