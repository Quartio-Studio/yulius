import { $f as f32, Ac as defineStd, Au as paramSphereSdf, Bd as ZERO, Cf as select, Cu as boxFaceUV, Du as paramBoxSdf, Ef as clamp, Eu as packedParamExtraFields, Ff as mix, Fu as torusSurfaceUV, Hd as call, Hf as vec2f, Ic as animatedTime, If as normalize, Iu as twistedRibbonSurfaceUV, Lf as pow, Mf as fract, Mu as paramTwistedRibbonSdf, Of as dot, Ou as paramCapsuleSdf, Pf as min, Pu as sphereSurfaceUV, Tu as mobiusSurfaceUV, Uf as vec3f, Wf as vec4f, Xf as struct, if as unpremultiplyAlpha, ju as paramTorusSdf, ku as paramMobiusSdf, mf as DEG_TO_RAD, np as __export, wf as abs, wu as capsuleSurfaceUV, xf as typegpu_default } from "./std-ClftfWpF.js";
import { c as transformPosition, s as transformEdges } from "./transformations-P50Uo_Ju.js";
var raymarch3d_exports = /* @__PURE__ */ __export({
	Distortion3DTrace: () => Distortion3DTrace,
	applyUVMode3d: () => applyUVMode3d$1,
	buildParamThreadedTrace: () => buildParamThreadedTrace$1,
	distortion3dLighting: () => distortion3dLighting$1
});
const Distortion3DTrace = /* @__PURE__ */ (globalThis.__TYPEGPU_AUTONAME__ ?? ((a) => a))(struct({
	hitPos: vec3f,
	normal: vec3f,
	rd: vec3f,
	hit: f32
}), "Distortion3DTrace");
const applyUVMode3d$1 = typegpu_default.fn([vec2f, f32], vec3f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((uv, uvMode) => {
	"use gpu";
	const stretched = clamp(uv, vec2f(0), vec2f(1));
	const mirrored = abs(fract(uv.mul(.5)).mul(2).sub(1));
	const wrapped = fract(uv);
	const tv1 = select(f32(0), f32(1), uv.y <= 1);
	const tv2 = select(f32(0), tv1, uv.y >= 0);
	const tv3 = select(f32(0), tv2, uv.x <= 1);
	const transparentVisible = select(f32(0), tv3, uv.x >= 0);
	const finalUV = select(select(select(wrapped, mirrored, uvMode < 2.5), uv, uvMode < 1.5), stretched, uvMode < .5);
	const visible = select(select(f32(1), transparentVisible, uvMode < 1.5), f32(1), uvMode < .5);
	return vec3f(finalUV.x, finalUV.y, visible);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [{
			"type": "i",
			"name": "uv"
		}, {
			"type": "i",
			"name": "uvMode"
		}],
		"body": [0, [
			[
				13,
				"stretched",
				[
					6,
					"std.clamp",
					[
						"uv",
						[
							6,
							"d.vec2f",
							[[5, "0"]]
						],
						[
							6,
							"d.vec2f",
							[[5, "1"]]
						]
					]
				]
			],
			[
				13,
				"mirrored",
				[
					6,
					"std.abs",
					[[
						6,
						[
							7,
							[
								6,
								[
									7,
									[
										6,
										"std.fract",
										[[
											6,
											[
												7,
												"uv",
												"mul"
											],
											[[5, "0.5"]]
										]]
									],
									"mul"
								],
								[[5, "2"]]
							],
							"sub"
						],
						[[5, "1"]]
					]]
				]
			],
			[
				13,
				"wrapped",
				[
					6,
					"std.fract",
					["uv"]
				]
			],
			[
				13,
				"tv1",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "0"]]
						],
						[
							6,
							"d.f32",
							[[5, "1"]]
						],
						[
							1,
							[
								7,
								"uv",
								"y"
							],
							"<=",
							[5, "1"]
						]
					]
				]
			],
			[
				13,
				"tv2",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "0"]]
						],
						"tv1",
						[
							1,
							[
								7,
								"uv",
								"y"
							],
							">=",
							[5, "0"]
						]
					]
				]
			],
			[
				13,
				"tv3",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "0"]]
						],
						"tv2",
						[
							1,
							[
								7,
								"uv",
								"x"
							],
							"<=",
							[5, "1"]
						]
					]
				]
			],
			[
				13,
				"transparentVisible",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "0"]]
						],
						"tv3",
						[
							1,
							[
								7,
								"uv",
								"x"
							],
							">=",
							[5, "0"]
						]
					]
				]
			],
			[
				13,
				"fuv2",
				[
					6,
					"std.select",
					[
						"wrapped",
						"mirrored",
						[
							1,
							"uvMode",
							"<",
							[5, "2.5"]
						]
					]
				]
			],
			[
				13,
				"fuv1",
				[
					6,
					"std.select",
					[
						"fuv2",
						"uv",
						[
							1,
							"uvMode",
							"<",
							[5, "1.5"]
						]
					]
				]
			],
			[
				13,
				"finalUV",
				[
					6,
					"std.select",
					[
						"fuv1",
						"stretched",
						[
							1,
							"uvMode",
							"<",
							[5, "0.5"]
						]
					]
				]
			],
			[
				13,
				"vis1",
				[
					6,
					"std.select",
					[
						[
							6,
							"d.f32",
							[[5, "1"]]
						],
						"transparentVisible",
						[
							1,
							"uvMode",
							"<",
							[5, "1.5"]
						]
					]
				]
			],
			[
				13,
				"visible",
				[
					6,
					"std.select",
					[
						"vis1",
						[
							6,
							"d.f32",
							[[5, "1"]]
						],
						[
							1,
							"uvMode",
							"<",
							[5, "0.5"]
						]
					]
				]
			],
			[10, [
				6,
				"d.vec3f",
				[
					[
						7,
						"finalUV",
						"x"
					],
					[
						7,
						"finalUV",
						"y"
					],
					"visible"
				]
			]]
		]]
	},
	externals: {
		"std.clamp": () => clamp,
		"d.vec2f": () => vec2f,
		"std.abs": () => abs,
		"std.fract": () => fract,
		"std.select": () => select,
		"d.f32": () => f32,
		"d.vec3f": () => vec3f
	}
}) && $.f)({}));
const distortion3dLighting$1 = typegpu_default.fn([
	vec4f,
	vec3f,
	vec3f,
	f32,
	f32,
	f32,
	f32
], vec4f)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((surfaceColor, normal, rd, hit, uvVisible, glossAmt, lightAmt) => {
	"use gpu";
	const lightDir = normalize(vec3f(.4, .7, -.6));
	const viewDir = rd.mul(-1);
	const diffuse = clamp(dot(normal, lightDir), 0, 1);
	const halfVec = normalize(lightDir.add(viewDir));
	const specPow = __tsover_add(__tsover_mul(glossAmt, 256), 4);
	const specular = __tsover_mul(__tsover_mul(pow(clamp(dot(normal, halfVec), 0, 1), specPow), glossAmt), 1.5);
	const nDotV = clamp(dot(normal, viewDir), 0, 1);
	const fresnel = __tsover_mul(__tsover_mul(pow(__tsover_sub(1, nDotV), 3), .5), glossAmt);
	const litClamped = clamp(mix(1, __tsover_add(__tsover_add(__tsover_add(.35, __tsover_mul(diffuse, .65)), specular), fresnel), lightAmt), 0, 3);
	const colClamped = clamp(surfaceColor.xyz.mul(litClamped), vec3f(0), vec3f(1));
	const alpha = __tsover_mul(__tsover_mul(hit, surfaceColor.w), uvVisible);
	return vec4f(colClamped.x, colClamped.y, colClamped.z, alpha);
}), {
	v: 2,
	name: void 0,
	ast: {
		"params": [
			{
				"type": "i",
				"name": "surfaceColor"
			},
			{
				"type": "i",
				"name": "normal"
			},
			{
				"type": "i",
				"name": "rd"
			},
			{
				"type": "i",
				"name": "hit"
			},
			{
				"type": "i",
				"name": "uvVisible"
			},
			{
				"type": "i",
				"name": "glossAmt"
			},
			{
				"type": "i",
				"name": "lightAmt"
			}
		],
		"body": [0, [
			[
				13,
				"lightDir",
				[
					6,
					"std.normalize",
					[[
						6,
						"d.vec3f",
						[
							[5, "0.4"],
							[5, "0.7"],
							[
								4,
								"-",
								[5, "0.6"]
							]
						]
					]]
				]
			],
			[
				13,
				"viewDir",
				[
					6,
					[
						7,
						"rd",
						"mul"
					],
					[[
						4,
						"-",
						[5, "1"]
					]]
				]
			],
			[
				13,
				"diffuse",
				[
					6,
					"std.clamp",
					[
						[
							6,
							"std.dot",
							["normal", "lightDir"]
						],
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"halfVec",
				[
					6,
					"std.normalize",
					[[
						6,
						[
							7,
							"lightDir",
							"add"
						],
						["viewDir"]
					]]
				]
			],
			[
				13,
				"specPow",
				[
					1,
					[
						1,
						"glossAmt",
						"*",
						[5, "256"]
					],
					"+",
					[5, "4"]
				]
			],
			[
				13,
				"specular",
				[
					1,
					[
						1,
						[
							6,
							"std.pow",
							[[
								6,
								"std.clamp",
								[
									[
										6,
										"std.dot",
										["normal", "halfVec"]
									],
									[5, "0"],
									[5, "1"]
								]
							], "specPow"]
						],
						"*",
						"glossAmt"
					],
					"*",
					[5, "1.5"]
				]
			],
			[
				13,
				"nDotV",
				[
					6,
					"std.clamp",
					[
						[
							6,
							"std.dot",
							["normal", "viewDir"]
						],
						[5, "0"],
						[5, "1"]
					]
				]
			],
			[
				13,
				"fresnel",
				[
					1,
					[
						1,
						[
							6,
							"std.pow",
							[[
								1,
								[5, "1"],
								"-",
								"nDotV"
							], [5, "3"]]
						],
						"*",
						[5, "0.5"]
					],
					"*",
					"glossAmt"
				]
			],
			[
				13,
				"ambient",
				[5, "0.35"]
			],
			[
				13,
				"lit",
				[
					6,
					"std.mix",
					[
						[5, "1"],
						[
							1,
							[
								1,
								[
									1,
									"ambient",
									"+",
									[
										1,
										"diffuse",
										"*",
										[5, "0.65"]
									]
								],
								"+",
								"specular"
							],
							"+",
							"fresnel"
						],
						"lightAmt"
					]
				]
			],
			[
				13,
				"litClamped",
				[
					6,
					"std.clamp",
					[
						"lit",
						[5, "0"],
						[5, "3"]
					]
				]
			],
			[
				13,
				"col",
				[
					6,
					[
						7,
						[
							7,
							"surfaceColor",
							"xyz"
						],
						"mul"
					],
					["litClamped"]
				]
			],
			[
				13,
				"colClamped",
				[
					6,
					"std.clamp",
					[
						"col",
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
			[
				13,
				"alpha",
				[
					1,
					[
						1,
						"hit",
						"*",
						[
							7,
							"surfaceColor",
							"w"
						]
					],
					"*",
					"uvVisible"
				]
			],
			[10, [
				6,
				"d.vec4f",
				[
					[
						7,
						"colClamped",
						"x"
					],
					[
						7,
						"colClamped",
						"y"
					],
					[
						7,
						"colClamped",
						"z"
					],
					"alpha"
				]
			]]
		]]
	},
	externals: {
		"std.normalize": () => normalize,
		"d.vec3f": () => vec3f,
		"std.clamp": () => clamp,
		"std.dot": () => dot,
		"std.pow": () => pow,
		"std.mix": () => mix,
		"d.vec4f": () => vec4f
	}
}) && $.f)({}));
function buildParamThreadedTrace$1(sdf, opts) {
	const STEPS = opts.steps, STEP_CAP = opts.stepCap, HIT_EPS = opts.hitEps, MAX_DIST = opts.maxDistance ?? 20;
	return typegpu_default.fn([
		vec2f,
		f32,
		vec2f,
		f32,
		vec4f,
		vec4f,
		vec4f,
		f32
	], Distortion3DTrace)(/* @__PURE__ */ (($) => (globalThis.__TYPEGPU_META__ ??= /* @__PURE__ */ new WeakMap()).set($.f = ((uv, aspect, center, zoom, rp0, rp1, rp2, t) => {
		"use gpu";
		const cx = center.x;
		const cy = __tsover_sub(1, center.y);
		const camDist = __tsover_sub(8, __tsover_mul(zoom, .06));
		const ro = vec3f(0, 0, __tsover_mul(camDist, -1));
		const rd = normalize(vec3f(__tsover_mul(__tsover_sub(uv.x, cx), aspect), __tsover_sub(uv.y, cy), 1.5));
		let rayT = f32(0);
		let hit = f32(0);
		const maxDistF = f32(MAX_DIST);
		for (let i = 0; i < STEPS; i++) {
			const dd = sdf(ro.add(rd.mul(rayT)), rp0, rp1, rp2, t);
			if (dd < HIT_EPS) {
				hit = f32(1);
				break;
			}
			if (rayT > maxDistF) break;
			rayT = __tsover_add(rayT, min(dd, STEP_CAP));
		}
		const hitPos = ro.add(rd.mul(rayT));
		const hh = .001;
		const hn = __tsover_mul(hh, -1);
		const k0 = sdf(hitPos.add(vec3f(hh, hn, hn)), rp0, rp1, rp2, t);
		const k1 = sdf(hitPos.add(vec3f(hn, hn, hh)), rp0, rp1, rp2, t);
		const k2 = sdf(hitPos.add(vec3f(hn, hh, hn)), rp0, rp1, rp2, t);
		const k3 = sdf(hitPos.add(vec3f(hh, hh, hh)), rp0, rp1, rp2, t);
		return Distortion3DTrace({
			hitPos,
			normal: normalize(vec3f(1, -1, -1).mul(k0).add(vec3f(-1, -1, 1).mul(k1)).add(vec3f(-1, 1, -1).mul(k2)).add(vec3f(1, 1, 1).mul(k3))),
			rd,
			hit
		});
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
					"name": "aspect"
				},
				{
					"type": "i",
					"name": "center"
				},
				{
					"type": "i",
					"name": "zoom"
				},
				{
					"type": "i",
					"name": "rp0"
				},
				{
					"type": "i",
					"name": "rp1"
				},
				{
					"type": "i",
					"name": "rp2"
				},
				{
					"type": "i",
					"name": "t"
				}
			],
			"body": [0, [
				[
					13,
					"cx",
					[
						7,
						"center",
						"x"
					]
				],
				[
					13,
					"cy",
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
				[
					13,
					"camDist",
					[
						1,
						[5, "8"],
						"-",
						[
							1,
							"zoom",
							"*",
							[5, "0.06"]
						]
					]
				],
				[
					13,
					"ro",
					[
						6,
						"d.vec3f",
						[
							[5, "0"],
							[5, "0"],
							[
								1,
								"camDist",
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
					"rd",
					[
						6,
						"std.normalize",
						[[
							6,
							"d.vec3f",
							[
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
										"cx"
									],
									"*",
									"aspect"
								],
								[
									1,
									[
										7,
										"uv",
										"y"
									],
									"-",
									"cy"
								],
								[5, "1.5"]
							]
						]]
					]
				],
				[
					12,
					"rayT",
					[
						6,
						"d.f32",
						[[5, "0"]]
					]
				],
				[
					12,
					"hit",
					[
						6,
						"d.f32",
						[[5, "0"]]
					]
				],
				[
					13,
					"maxDistF",
					[
						6,
						"d.f32",
						["MAX_DIST"]
					]
				],
				[
					14,
					[
						12,
						"i",
						[5, "0"]
					],
					[
						1,
						"i",
						"<",
						"STEPS"
					],
					[
						102,
						"++",
						"i"
					],
					[0, [
						[
							13,
							"p",
							[
								6,
								[
									7,
									"ro",
									"add"
								],
								[[
									6,
									[
										7,
										"rd",
										"mul"
									],
									["rayT"]
								]]
							]
						],
						[
							13,
							"dd",
							[
								6,
								"sdf",
								[
									"p",
									"rp0",
									"rp1",
									"rp2",
									"t"
								]
							]
						],
						[
							11,
							[
								1,
								"dd",
								"<",
								"HIT_EPS"
							],
							[0, [[
								2,
								"hit",
								"=",
								[
									6,
									"d.f32",
									[[5, "1"]]
								]
							], [17]]]
						],
						[
							11,
							[
								1,
								"rayT",
								">",
								"maxDistF"
							],
							[0, [[17]]]
						],
						[
							2,
							"rayT",
							"=",
							[
								1,
								"rayT",
								"+",
								[
									6,
									"std.min",
									["dd", "STEP_CAP"]
								]
							]
						]
					]]
				],
				[
					13,
					"hitPos",
					[
						6,
						[
							7,
							"ro",
							"add"
						],
						[[
							6,
							[
								7,
								"rd",
								"mul"
							],
							["rayT"]
						]]
					]
				],
				[
					13,
					"hh",
					[5, "0.001"]
				],
				[
					13,
					"hn",
					[
						1,
						"hh",
						"*",
						[
							4,
							"-",
							[5, "1"]
						]
					]
				],
				[
					13,
					"k0",
					[
						6,
						"sdf",
						[
							[
								6,
								[
									7,
									"hitPos",
									"add"
								],
								[[
									6,
									"d.vec3f",
									[
										"hh",
										"hn",
										"hn"
									]
								]]
							],
							"rp0",
							"rp1",
							"rp2",
							"t"
						]
					]
				],
				[
					13,
					"k1",
					[
						6,
						"sdf",
						[
							[
								6,
								[
									7,
									"hitPos",
									"add"
								],
								[[
									6,
									"d.vec3f",
									[
										"hn",
										"hn",
										"hh"
									]
								]]
							],
							"rp0",
							"rp1",
							"rp2",
							"t"
						]
					]
				],
				[
					13,
					"k2",
					[
						6,
						"sdf",
						[
							[
								6,
								[
									7,
									"hitPos",
									"add"
								],
								[[
									6,
									"d.vec3f",
									[
										"hn",
										"hh",
										"hn"
									]
								]]
							],
							"rp0",
							"rp1",
							"rp2",
							"t"
						]
					]
				],
				[
					13,
					"k3",
					[
						6,
						"sdf",
						[
							[
								6,
								[
									7,
									"hitPos",
									"add"
								],
								[[
									6,
									"d.vec3f",
									[
										"hh",
										"hh",
										"hh"
									]
								]]
							],
							"rp0",
							"rp1",
							"rp2",
							"t"
						]
					]
				],
				[
					13,
					"normal",
					[
						6,
						"std.normalize",
						[[
							6,
							[
								7,
								[
									6,
									[
										7,
										[
											6,
											[
												7,
												[
													6,
													[
														7,
														[
															6,
															"d.vec3f",
															[
																[5, "1"],
																[
																	4,
																	"-",
																	[5, "1"]
																],
																[
																	4,
																	"-",
																	[5, "1"]
																]
															]
														],
														"mul"
													],
													["k0"]
												],
												"add"
											],
											[[
												6,
												[
													7,
													[
														6,
														"d.vec3f",
														[
															[
																4,
																"-",
																[5, "1"]
															],
															[
																4,
																"-",
																[5, "1"]
															],
															[5, "1"]
														]
													],
													"mul"
												],
												["k1"]
											]]
										],
										"add"
									],
									[[
										6,
										[
											7,
											[
												6,
												"d.vec3f",
												[
													[
														4,
														"-",
														[5, "1"]
													],
													[5, "1"],
													[
														4,
														"-",
														[5, "1"]
													]
												]
											],
											"mul"
										],
										["k2"]
									]]
								],
								"add"
							],
							[[
								6,
								[
									7,
									[
										6,
										"d.vec3f",
										[
											[5, "1"],
											[5, "1"],
											[5, "1"]
										]
									],
									"mul"
								],
								["k3"]
							]]
						]]
					]
				],
				[10, [
					6,
					"Distortion3DTrace",
					[[104, {
						"hitPos": "hitPos",
						"normal": "normal",
						"rd": "rd",
						"hit": "hit"
					}]]
				]]
			]]
		},
		externals: {
			"d.vec3f": () => vec3f,
			"std.normalize": () => normalize,
			"d.f32": () => f32,
			"MAX_DIST": () => MAX_DIST,
			"STEPS": () => STEPS,
			"sdf": () => sdf,
			"HIT_EPS": () => HIT_EPS,
			"std.min": () => min,
			"STEP_CAP": () => STEP_CAP,
			"Distortion3DTrace": () => Distortion3DTrace
		}
	}) && $.f)({}));
}
var { applyUVMode3d, distortion3dLighting, buildParamThreadedTrace } = raymarch3d_exports;
var DEG = DEG_TO_RAD;
var TAU_RATE = Math.PI * 2 / 10;
var DEFAULT_RIBBON = JSON.stringify({
	type: "ribbon",
	angle: 0,
	twist: 50,
	width: 40,
	thickness: 20,
	seed: 0
});
var SHAPES = {
	sphere: {
		sdf: paramSphereSdf,
		surfaceUV: sphereSurfaceUV,
		uvHint: "sphereSurfaceUV",
		steps: 64,
		stepCap: 1.5,
		hitEps: .001
	},
	torus: {
		sdf: paramTorusSdf,
		surfaceUV: torusSurfaceUV,
		uvHint: "torusSurfaceUV",
		steps: 96,
		stepCap: .5,
		hitEps: .001
	},
	box: {
		sdf: paramBoxSdf,
		surfaceUV: boxFaceUV,
		uvHint: "boxFaceUV",
		steps: 96,
		stepCap: .5,
		hitEps: .001
	},
	capsule: {
		sdf: paramCapsuleSdf,
		surfaceUV: capsuleSurfaceUV,
		uvHint: "capsuleSurfaceUV",
		steps: 64,
		stepCap: 1.5,
		hitEps: .001
	},
	mobius: {
		sdf: paramMobiusSdf,
		surfaceUV: mobiusSurfaceUV,
		uvHint: "mobiusSurfaceUV",
		steps: 96,
		stepCap: .3,
		hitEps: .001
	},
	ribbon: {
		sdf: paramTwistedRibbonSdf,
		surfaceUV: twistedRibbonSurfaceUV,
		uvHint: "twistedRibbonSurfaceUV",
		steps: 128,
		stepCap: .08,
		hitEps: .002
	}
};
var SPIN_DEFAULTS = {
	torus: {
		rotX: -90,
		spinY: .05
	},
	box: {
		rotX: 15,
		spinY: .1
	},
	sphere: { spinY: .1 },
	capsule: { spinY: .1 },
	mobius: {
		rotX: -30,
		spinY: .05
	}
};
const componentDefinition = defineStd({
	name: "Form3D",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Distortions",
	description: "Wraps child content onto a 3D raymarched shape with lighting.",
	requiresRTT: true,
	requiresChild: true,
	animatedTime: { speed: "speed" },
	extraFields: packedParamExtraFields("_f3p", [
		[
			1,
			0,
			1,
			0
		],
		[
			1,
			0,
			.35,
			.1
		],
		[
			.3,
			.02,
			0,
			0
		]
	]),
	props: {
		shape3d: {
			default: DEFAULT_RIBBON,
			description: "3D shape and its parameters",
			ui: {
				type: "shape3d",
				label: "Shape",
				group: "Shape"
			}
		},
		shape3dType: {
			default: "ribbon",
			compileTime: true,
			description: "Active shape type — triggers recompile when shape is switched"
		},
		center: {
			default: {
				x: .5,
				y: .5
			},
			transform: transformPosition,
			description: "Center position of the shape on screen",
			ui: {
				type: "position",
				label: "Center",
				group: "Position"
			}
		},
		zoom: {
			default: 50,
			description: "Camera zoom level",
			ui: {
				type: ["range", "map"],
				min: 10,
				max: 200,
				step: 1,
				label: "Zoom",
				group: "Scene"
			}
		},
		glossiness: {
			default: 50,
			description: "Specular highlight intensity and sharpness",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 200,
				step: 1,
				label: "Glossiness",
				group: "Lighting"
			}
		},
		lighting: {
			default: 50,
			description: "Overall intensity of lighting effects",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 200,
				step: 1,
				label: "Lighting",
				group: "Lighting"
			}
		},
		uvMode: {
			default: "stretch",
			transform: transformEdges,
			description: "How to handle UV coordinates at shape boundaries",
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
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
				label: "UV Edges",
				group: "Scene"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed — scales all spin rates",
			ui: {
				type: "range",
				min: -10,
				max: 10,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		}
	},
	gpu: { fragment: (params) => {
		const { childNode, convertToTexture, uniforms, propValues, getCpuValue, onBeforeRender, ctx } = params;
		if (!childNode) return ZERO;
		const childTexture = convertToTexture(childNode);
		const shapeType = propValues.shape3dType || "ribbon";
		const shape = SHAPES[shapeType] ?? SHAPES.ribbon;
		let spinX = 0, spinY = 0, spinZ = 0;
		let lastJson = "";
		let cfg = (() => {
			try {
				return JSON.parse(getCpuValue("shape3d"));
			} catch {
				return {};
			}
		})();
		const num = (k, f) => typeof cfg[k] === "number" ? cfg[k] : f;
		const drive = (deltaTime) => {
			const raw = getCpuValue("shape3d");
			if (typeof raw === "string") {
				if (raw !== lastJson) {
					lastJson = raw;
					try {
						cfg = JSON.parse(raw);
					} catch {}
				}
			} else if (raw && typeof raw === "object") cfg = raw;
			const speed = getCpuValue("speed") ?? 1;
			if (shapeType === "ribbon") {
				const angle = num("angle", 0) * DEG;
				const twist = num("twist", 50) * .03;
				const width = num("width", 40) * .012;
				const halfW = width * .5;
				const thickness = num("thickness", 20) * .001 + .01;
				const seed = num("seed", 0);
				const lipschitz = Math.max(halfW * twist, 1);
				params.setExtraField("_f3p0", [
					angle,
					twist,
					halfW,
					width
				]);
				params.setExtraField("_f3p1", [
					thickness,
					seed,
					lipschitz,
					0
				]);
				params.setExtraField("_f3p2", [
					0,
					0,
					0,
					0
				]);
				return;
			}
			const dfl = SPIN_DEFAULTS[shapeType] ?? {};
			spinX += deltaTime * speed * num("spinX", dfl.spinX ?? 0) * TAU_RATE;
			spinY += deltaTime * speed * num("spinY", dfl.spinY ?? 0) * TAU_RATE;
			spinZ += deltaTime * speed * num("spinZ", dfl.spinZ ?? 0) * TAU_RATE;
			const aX = num("rotX", dfl.rotX ?? 0) * DEG + spinX;
			const aY = num("rotY", dfl.rotY ?? 0) * DEG + spinY;
			const aZ = num("rotZ", dfl.rotZ ?? 0) * DEG + spinZ;
			const rot0 = [
				Math.cos(aX),
				Math.sin(aX),
				Math.cos(aY),
				Math.sin(aY)
			];
			const cz = Math.cos(aZ), sz = Math.sin(aZ);
			if (shapeType === "sphere") {
				params.setExtraField("_f3p0", rot0);
				params.setExtraField("_f3p1", [
					cz,
					sz,
					num("radius", 60) * .008 + .1,
					0
				]);
				params.setExtraField("_f3p2", [
					0,
					0,
					0,
					0
				]);
			} else if (shapeType === "torus") {
				params.setExtraField("_f3p0", rot0);
				params.setExtraField("_f3p1", [
					cz,
					sz,
					num("outerRadius", 60) * .008 + .1,
					num("tubeRadius", 25) * .006 + .05
				]);
				params.setExtraField("_f3p2", [
					0,
					0,
					0,
					0
				]);
			} else if (shapeType === "box") {
				params.setExtraField("_f3p0", rot0);
				params.setExtraField("_f3p1", [
					cz,
					sz,
					num("sizeX", 50) * .006,
					num("sizeY", 50) * .006
				]);
				params.setExtraField("_f3p2", [
					num("sizeZ", 50) * .006,
					num("rounding", 20) * .001,
					0,
					0
				]);
			} else if (shapeType === "capsule") {
				params.setExtraField("_f3p0", rot0);
				params.setExtraField("_f3p1", [
					cz,
					sz,
					num("radius", 25) * .006 + .05,
					num("height", 80) * .005
				]);
				params.setExtraField("_f3p2", [
					0,
					0,
					0,
					0
				]);
			} else {
				params.setExtraField("_f3p0", rot0);
				params.setExtraField("_f3p1", [
					cz,
					sz,
					num("ringRadius", 60) * .008 + .1,
					num("halfWidth", 30) * .005
				]);
				params.setExtraField("_f3p2", [
					num("thickness", 8) * .001 + .005,
					0,
					0,
					0
				]);
			}
		};
		drive(0);
		onBeforeRender((fp) => drive(fp?.deltaTime ?? 0));
		const t = animatedTime(params);
		const traceRes = call(buildParamThreadedTrace(shape.sdf, {
			steps: shape.steps,
			stepCap: shape.stepCap,
			hitEps: shape.hitEps
		}), `form3dTrace_${shapeType}`, [
			ctx.uv,
			ctx.aspect,
			uniforms.center,
			uniforms.zoom,
			uniforms._f3p0,
			uniforms._f3p1,
			uniforms._f3p2,
			t
		]);
		const hitPos = traceRes.member("hitPos");
		const normal = traceRes.member("normal");
		const rd = traceRes.member("rd");
		const hit = traceRes.member("hit");
		const uvm = call(applyUVMode3d, "applyUVMode3d", [call(shape.surfaceUV, shape.uvHint, [
			hitPos,
			normal,
			uniforms._f3p0,
			uniforms._f3p1,
			uniforms._f3p2,
			t
		]), uniforms.uvMode]);
		const finalUV = uvm.member("xy");
		const uvVisible = uvm.member("z");
		return call(distortion3dLighting, "distortion3dLighting", [
			call(unpremultiplyAlpha, "unpremultiplyAlpha", [childTexture.sample(finalUV)]),
			normal,
			rd,
			hit,
			uvVisible,
			uniforms.glossiness.mul(.005),
			uniforms.lighting.mul(.005)
		]);
	} }
});
var Form3D_default = componentDefinition;
export { componentDefinition as n, Form3D_default as t };
