import { Ac as defineStd, Al as uniformOf, Da as glowAt, Dc as vec3, Gs as clamp, Ha as vignetteMask, Ml as p, Oa as glowSpot, Oc as vec4, Sc as splat3, Tc as sub, Wf as vec4f, Ws as add, Ys as exp, Zs as float, bc as smoothstep, cc as max, dc as mul, dr as surfaceOf, fc as neg, oc as local, qs as div, uc as mix, ws as cpuHash01 } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var TAU = Math.PI * 2;
const componentDefinition = defineStd({
	name: "StudioBackground",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Multi-light studio background with ambient motion.",
	extraFields: {
		ambPosA: {
			schema: vec4f,
			initial: [
				0,
				0,
				0,
				0
			]
		},
		ambPosB: {
			schema: vec4f,
			initial: [
				0,
				0,
				0,
				0
			]
		},
		ambSizeSq: {
			schema: vec4f,
			initial: [
				.01,
				.01,
				.01,
				.01
			]
		}
	},
	props: {
		color: {
			default: "#d8dbec",
			transform: transformColor,
			description: "Base studio surface color",
			ui: {
				type: "color",
				label: "Surface Color",
				group: "Colors"
			}
		},
		keyColor: {
			default: "#d5e4ea",
			transform: transformColor,
			description: "Color of the overhead key light",
			ui: {
				type: "color",
				label: "Key Color",
				group: "Key Light"
			}
		},
		keyIntensity: {
			default: 40,
			description: "Intensity of the key light",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Key Light"
			}
		},
		keySoftness: {
			default: 50,
			description: "How diffuse the key light is",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Softness",
				group: "Key Light"
			}
		},
		fillColor: {
			default: "#d5e4ea",
			transform: transformColor,
			description: "Color of the side fill lights",
			ui: {
				type: "color",
				label: "Fill Color",
				group: "Fill Lights"
			}
		},
		fillIntensity: {
			default: 10,
			description: "Intensity of the fill lights",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Fill Lights"
			}
		},
		fillSoftness: {
			default: 70,
			description: "How diffuse the fill lights are",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Softness",
				group: "Fill Lights"
			}
		},
		fillAngle: {
			default: 70,
			description: "How far apart the fill lights are from center",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Spread",
				group: "Fill Lights"
			}
		},
		backColor: {
			default: "#c8d4e8",
			transform: transformColor,
			description: "Color of the upward back wash",
			ui: {
				type: "color",
				label: "Back Color",
				group: "Back Light"
			}
		},
		backIntensity: {
			default: 20,
			description: "Intensity of the back wash",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Back Light"
			}
		},
		backSoftness: {
			default: 80,
			description: "How diffuse the back wash is",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Softness",
				group: "Back Light"
			}
		},
		brightness: {
			default: 20,
			description: "Overall ambient light level",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Ambient",
				group: "Lighting"
			}
		},
		vignette: {
			default: 0,
			description: "Edge darkening",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Vignette",
				group: "Lighting"
			}
		},
		center: {
			default: {
				x: .5,
				y: .8
			},
			transform: transformPosition,
			description: "Where the spotlight meets the floor",
			ui: {
				type: "position",
				label: "Stage Center",
				group: "Scene"
			}
		},
		lightTarget: {
			default: 100,
			description: "How far toward the floor vs wall the spotlights aim",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Light Depth",
				group: "Scene"
			}
		},
		wallCurvature: {
			default: 10,
			description: "How rounded the cove is",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Wall Curvature",
				group: "Scene"
			}
		},
		ambientIntensity: {
			default: 50,
			description: "Intensity of drifting ambient lights",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Ambient Detail",
				group: "Environment"
			}
		},
		ambientSpeed: {
			default: 2,
			description: "Drift speed",
			ui: {
				type: "range",
				min: -5,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Environment"
			}
		},
		seed: {
			default: 0,
			description: "Seed for ambient pattern",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Environment"
			}
		}
	},
	paint: (params) => {
		const { uniforms } = params;
		const { uv, viewport } = surfaceOf(params);
		const aspect = local(div(viewport.member("x"), max(viewport.member("y"), 1e-6)), "aspect");
		const u = (name) => uniformOf(p(name), params);
		const brightness = local(mul(u("brightness"), .02), "sbBrightness");
		const keyInt = local(mul(u("keyIntensity"), .015), "sbKeyInt");
		const fillInt = local(mul(u("fillIntensity"), .015), "sbFillInt");
		const fillOffset = mul(u("fillAngle"), .005);
		const backInt = mul(u("backIntensity"), .015);
		const lightTarget = local(mul(u("lightTarget"), .01), "sbTarget");
		const curvature = local(mul(u("wallCurvature"), .01), "sbCurvature");
		const keyDecay = mix(5, .8, mul(u("keySoftness"), .01));
		const fillDecay = mix(5, .8, mul(u("fillSoftness"), .01));
		const backDecay = local(mix(5, .8, mul(u("backSoftness"), .01)), "sbBackDecay");
		const centerX = local(u("center").member("x"), "coveCenterX");
		const centerY = local(sub(1, u("center").member("y")), "coveCenterY");
		const transitionWidth = local(mix(.015, .15, curvature), "coveTransition");
		const isFloor = local(smoothstep(sub(centerY, transitionWidth), add(centerY, transitionWidth), uv.member("y")), "coveIsFloor");
		const notFloor = local(sub(1, isFloor), "coveNotFloor");
		const wallNormY = local(clamp(div(uv.member("y"), max(centerY, 1e-6)), 0, 1), "coveWallY");
		const wallNormY2 = local(mul(wallNormY, wallNormY), "coveWallY2");
		const floorNormY = clamp(div(sub(uv.member("y"), centerY), max(sub(1, centerY), 1e-6)), 0, 1);
		const uvXCentered = local(mul(sub(uv.member("x"), centerX), aspect), "coveX");
		const coneWidth = local(mix(.05, .45, wallNormY2), "coveCone");
		const wallFallY = mul(sub(1, wallNormY), 1.2);
		const wallFallY2 = local(mul(wallFallY, wallFallY), "coveWallFall");
		const floorPoolY = add(centerY, mul(sub(1, centerY), .3));
		const floorDy = mul(sub(uv.member("y"), floorPoolY), 4);
		const floorDy2 = local(mul(floorDy, floorDy), "coveFloorFall");
		const ambient = mul(mix(mix(.2, .4, wallNormY), mix(.38, .18, floorNormY), isFloor), brightness);
		const curveDist = div(sub(uv.member("y"), centerY), add(transitionWidth, .001));
		const neutralBase = local(add(ambient, mul(mul(mul(exp(mul(mul(-.5, curveDist), curveDist)), curvature), brightness), .1)), "sbBase");
		const wallBias = local(add(mul(sub(1, lightTarget), 1.4), .3), "sbWallBias");
		const floorBias = local(add(mul(lightTarget, 1.4), .3), "sbFloorBias");
		const downSpot = (sourceX, decay) => {
			const beamX = mix(sourceX, centerX, wallNormY2);
			const wDx = div(mul(sub(uv.member("x"), beamX), aspect), coneWidth);
			const wall = mul(mul(exp(mul(neg(add(mul(wDx, wDx), wallFallY2)), decay)), notFloor), wallBias);
			const poolX = mix(centerX, sourceX, .25);
			const fDx = mul(mul(sub(uv.member("x"), poolX), aspect), 1.4);
			return add(wall, mul(mul(exp(mul(neg(add(mul(fDx, fDx), floorDy2)), mul(decay, 1.5))), isFloor), floorBias));
		};
		let light = local(vec3(neutralBase, neutralBase, neutralBase), "sbLight0");
		light = add(light, mul(u("keyColor").member("rgb"), mul(local(downSpot(centerX, keyDecay), "sbKey"), keyInt)));
		const fills = add(mul(local(downSpot(sub(centerX, fillOffset), fillDecay), "sbFillL"), fillInt), mul(local(downSpot(add(centerX, fillOffset), fillDecay), "sbFillR"), fillInt));
		light = add(light, mul(u("fillColor").member("rgb"), fills));
		const backFloor = mul(glowSpot(mul(uvXCentered, .5), mul(sub(uv.member("y"), mix(1, centerY, .15)), 4), backDecay), isFloor);
		const backWall = mul(mul(glowSpot(mul(uvXCentered, .6), mul(sub(centerY, uv.member("y")), 3), mul(backDecay, .8)), notFloor), wallNormY2);
		light = add(light, mul(u("backColor").member("rgb"), mul(add(backFloor, backWall), backInt)));
		const bounce = mul(mul(glowSpot(mul(uvXCentered, .7), mul(add(sub(uv.member("y"), centerY), .02), 10), float(2.5)), mul(keyInt, .12)), notFloor);
		light = add(light, splat3(bounce));
		const ambAnchorY = sub(centerY, .2);
		const orbits = add(add(add(glowAt({
			uv,
			aspect,
			x: add(centerX, uniforms.ambPosA.member("x")),
			y: add(ambAnchorY, uniforms.ambPosA.member("y")),
			sizeSq: uniforms.ambSizeSq.member("x")
		}), glowAt({
			uv,
			aspect,
			x: add(centerX, uniforms.ambPosA.member("z")),
			y: add(ambAnchorY, uniforms.ambPosA.member("w")),
			sizeSq: uniforms.ambSizeSq.member("y")
		})), glowAt({
			uv,
			aspect,
			x: add(centerX, uniforms.ambPosB.member("x")),
			y: add(ambAnchorY, uniforms.ambPosB.member("y")),
			sizeSq: uniforms.ambSizeSq.member("z")
		})), glowAt({
			uv,
			aspect,
			x: add(centerX, uniforms.ambPosB.member("z")),
			y: add(ambAnchorY, uniforms.ambPosB.member("w")),
			sizeSq: uniforms.ambSizeSq.member("w")
		}));
		light = add(light, splat3(mul(orbits, mul(mul(u("ambientIntensity"), .012), .25))));
		let ambTime = 0;
		let lastSeed = NaN;
		let orbitConsts = [];
		params.onBeforeRender((fp) => {
			ambTime += (fp?.deltaTime ?? 0) * (params.getCpuValue("ambientSpeed") ?? 2);
			const seed = params.getCpuValue("seed") ?? 0;
			if (seed !== lastSeed) {
				lastSeed = seed;
				orbitConsts = [];
				for (let i = 0; i < 4; i++) {
					const sd = seed + i * 73.1;
					const sizeVal = cpuHash01(sd + 311) * .12 + .1;
					orbitConsts.push({
						sizeSq: sizeVal * sizeVal,
						radiusX: cpuHash01(sd) * .5 + .2,
						radiusY: cpuHash01(sd + 37) * .25 + .1,
						freqX: cpuHash01(sd + 91) * .3 + .1,
						freqY: cpuHash01(sd + 143) * .25 + .08,
						phaseX: cpuHash01(sd + 200) * TAU,
						phaseY: cpuHash01(sd + 257) * TAU
					});
				}
			}
			const off = orbitConsts.map((c) => [Math.sin(ambTime * c.freqX + c.phaseX) * c.radiusX, Math.cos(ambTime * c.freqY + c.phaseY) * c.radiusY]);
			params.setExtraField("ambPosA", [
				off[0][0],
				off[0][1],
				off[1][0],
				off[1][1]
			]);
			params.setExtraField("ambPosB", [
				off[2][0],
				off[2][1],
				off[3][0],
				off[3][1]
			]);
			params.setExtraField("ambSizeSq", [
				orbitConsts[0].sizeSq,
				orbitConsts[1].sizeSq,
				orbitConsts[2].sizeSq,
				orbitConsts[3].sizeSq
			]);
		});
		const vig = vignetteMask({
			strength: p("vignette"),
			scale: .025
		})(params);
		return vec4(clamp(mul(mul(u("color").member("rgb"), clamp(light, splat3(0), splat3(3))), vig), splat3(0), splat3(1)), 1);
	}
});
var StudioBackground_default = componentDefinition;
export { componentDefinition as n, StudioBackground_default as t };
