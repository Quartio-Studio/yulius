import { $r as keyLightAt, Ac as defineStd, Bd as ZERO, Ci as surfaceNoiseAt, Dc as vec3, Ec as vec2, Fr as beerLambert, Gs as clamp, Hr as fieldSlope, Ic as animatedTime, Js as dot, Oc as vec4, Ru as shapeEffectBoundingBoxDeclaration, Sc as splat3, Tc as sub, Ur as flowWarp, Us as abs, Ws as add, Yr as guarded, Zr as insideShape, Zs as float, ac as length, ar as direction, bc as smoothstep, ci as opticalThickness, dc as mul, gc as refract, gi as sharpGlint, hc as reflect, hi as shapedSurface, mc as pow, mi as schlickFresnel, oc as local, pc as normalize, qr as grazingFlat, qs as div, ri as marchedNormal, uc as mix, vi as silhouette, wi as surfacePattern, xi as surfaceField, yc as sin } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
var DEFAULT_SHAPE_CONFIG = JSON.stringify({
	type: "sphere3D",
	radius: .35
});
var waterFbm = (uvE, hint) => {
	const uv = local(uvE, hint);
	const n0 = surfaceNoiseAt(uv);
	const n1 = mul(surfaceNoiseAt(add(mul(uv, 2.07), vec2(4.3, 9.1))), .5);
	const n2 = mul(surfaceNoiseAt(add(mul(uv, 4.13), vec2(1.7, 8.2))), .25);
	return mul(add(add(n0, n1), n2), .52);
};
var waterNoise1 = (uv) => mul(surfaceNoiseAt(uv), .52);
var waterSky = (rx, ry, lx, ly, envC, envS, sharp, hint) => {
	const ex = local(sub(mul(rx, envC), mul(ry, envS)), `${hint}Ex`);
	const ey = local(add(mul(rx, envS), mul(ry, envC)), `${hint}Ey`);
	const up = local(clamp(add(mul(ey, .5), .5), 0, 1), `${hint}Up`);
	const horizon = vec3(.9, .96, 1);
	const zenith = vec3(.2, .46, .85);
	const base = mix(vec3(.04, .1, .16), mix(horizon, zenith, splat3(smoothstep(.5, 1, up))), splat3(smoothstep(.4, .55, up)));
	const sunCenter = vec2(mul(lx, .8), add(mul(ly, .8), .12));
	const sunRad = local(mix(.5, .13, sharp), `${hint}SunRad`);
	const sd = length(sub(vec2(ex, ey), sunCenter));
	const sun = smoothstep(sunRad, mul(sunRad, .15), sd);
	return add(base, mul(vec3(1, .97, .88), mul(sun, 1.5)));
};
const componentDefinition = defineStd({
	name: "Water",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: shapeEffectBoundingBoxDeclaration,
	category: "Shape Effects",
	description: "Translucent water — a refractive, light-absorbing body that deepens to its color with thickness, wrapped in a wind-driven wave surface that reflects a procedural sky, with Fresnel-bright grazing edges and foam breaking on the crests and shoreline",
	animatedTime: { speed: "speed" },
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
			description: "Center position of the water shape",
			ui: {
				type: "position",
				label: "Center",
				group: "Position",
				units: ["%", "px"]
			}
		},
		scale: {
			default: 1,
			description: "Scale of the water shape (1 = default size)",
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
			description: "Rotation of the water shape in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Rotation",
				group: "Position"
			}
		},
		waterColor: {
			default: "#00bfeb",
			transform: transformColor,
			description: "The color the water saturates toward with depth",
			ui: {
				type: "color",
				label: "Water Color",
				group: "Water"
			}
		},
		clarity: {
			default: .4,
			description: "How much of the reflected sky transmits through the body",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Clarity",
				group: "Water"
			}
		},
		depth: {
			default: 5,
			description: "How quickly the water darkens and saturates toward its color with thickness",
			ui: {
				type: "range",
				min: 0,
				max: 8,
				step: .01,
				label: "Depth",
				group: "Water"
			}
		},
		shallows: {
			default: .5,
			description: "Brightness of the thin shallow water at the edges — high fades the rim to light translucent water, low keeps the edges close to the deep water color",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Shallows",
				group: "Water"
			}
		},
		caustics: {
			default: .15,
			description: "Intensity of the light caustics bubbling up through the body — bright veins of focused light churning inside the water",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Caustics",
				group: "Water"
			}
		},
		causticScale: {
			default: 6,
			description: "Size of the caustic veins — higher = finer, busier light patterns inside the body",
			ui: {
				type: "range",
				min: 1,
				max: 20,
				step: .1,
				label: "Caustic Scale",
				group: "Water"
			}
		},
		choppiness: {
			default: .8,
			description: "Height of the wind-driven waves — the depth of the folds that bend the reflection",
			ui: {
				type: "range",
				min: 0,
				max: 1.5,
				step: .01,
				label: "Choppiness",
				group: "Waves"
			}
		},
		waveScale: {
			default: 3.5,
			description: "Scale of the wave fronts — higher = smaller, busier ripples",
			ui: {
				type: "range",
				min: .5,
				max: 8,
				step: .01,
				label: "Wave Scale",
				group: "Waves"
			}
		},
		swirl: {
			default: .6,
			description: "Swirl of the flow — domain-warps the wave fronts into curling eddies",
			ui: {
				type: "range",
				min: 0,
				max: 1.5,
				step: .01,
				label: "Swirl",
				group: "Waves"
			}
		},
		speed: {
			default: .5,
			description: "Speed of the rolling waves. 0 pauses the surface.",
			ui: {
				type: "range",
				min: 0,
				max: 4,
				step: .01,
				label: "Speed",
				group: "Waves"
			}
		},
		reflection: {
			default: 2,
			description: "Strength of the reflected sky across the surface",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Reflection",
				group: "Surface"
			}
		},
		envRotation: {
			default: 0,
			description: "Rotates the reflected sky — spins where the bright sky and sun fall",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Env Rotation",
				group: "Surface"
			}
		},
		sharpness: {
			default: .1,
			description: "Tightness of the sun glint and reflected sun",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Sharpness",
				group: "Surface"
			}
		},
		lightAngle: {
			default: 30,
			description: "Direction of the sun, in degrees — drives the glint and where the sun reflects",
			ui: {
				type: "range",
				min: 0,
				max: 360,
				step: 1,
				label: "Light Angle",
				group: "Surface"
			}
		},
		foam: {
			default: .3,
			description: "White foam breaking on the wave crests and along the shoreline edge",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Foam",
				group: "Surface"
			}
		},
		edgeSoftness: {
			default: .05,
			description: "Softness of the shape boundary edge",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Softness",
				group: "Surface"
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
			description: "URL to a pre-generated SDF .bin file"
		},
		shapeType: {
			default: "",
			compileTime: true,
			description: "Active SDF shape type"
		}
	},
	...shapedSurface({
		pattern: "raw",
		surface: (frame, params) => {
			const { uniforms: u } = params;
			const t = animatedTime(params);
			const vol = frame.volumetric;
			const field = surfaceField(frame, params, { scale: u.scale });
			const { sdf, pxH } = field;
			const light = direction(u.lightAngle, "light");
			const lx = light.member("x");
			const ly = light.member("y");
			const env = direction(u.envRotation, "env");
			const envC = env.member("x");
			const envS = env.member("y");
			const grad = fieldSlope(field);
			let nGeo;
			if (vol) nGeo = marchedNormal(field);
			else {
				const rimBevel = local(mul(sub(1, clamp(div(mul(sdf, -1), .05), 0, 1)), 1.6), "rimBevel");
				nGeo = local(normalize(vec3(mul(grad.member("x"), rimBevel), mul(grad.member("y"), rimBevel), -1)), "nGeo");
			}
			const pat = surfacePattern(frame, field);
			const fx = pat.member("x");
			const fy = pat.member("y");
			const freq = local(mul(u.waveScale, 2.4), "freq");
			const flowT = local(mul(t, .2), "flowT");
			const q = local(vec2(mul(mul(fx, freq), .8), mul(mul(fy, freq), 1.25)), "q");
			const wAmt = mul(u.swirl, .9);
			const qw = local(add(add(q, mul(flowWarp(mul(q, .6), flowT), wAmt)), vec2(0, mul(flowT, -1))), "qw");
			const re = .06;
			const h0 = local(waterFbm(qw, "fbmH0"), "h0");
			const h0n1 = local(waterNoise1(qw), "h0n1");
			const hx = clamp(div(sub(waterNoise1(add(qw, vec2(re, 0))), h0n1), re), -4, 4);
			const hy = clamp(div(sub(waterNoise1(add(qw, vec2(0, re))), h0n1), re), -4, 4);
			const fineRe = .025;
			const qf = local(add(add(mul(qw, 3.3), vec2(mul(flowT, 1.7), mul(flowT, -1.15))), vec2(11.3, 4.7)), "qf");
			const f0n1 = local(waterNoise1(qf), "f0n1");
			const fxg = clamp(div(sub(waterNoise1(add(qf, vec2(fineRe, 0))), f0n1), fineRe), -4, 4);
			const fyg = clamp(div(sub(waterNoise1(add(qf, vec2(0, fineRe))), f0n1), fineRe), -4, 4);
			const relief = local(mul(u.choppiness, .5), "relief");
			const reliefFine = local(mul(u.choppiness, .22), "reliefFine");
			const n = local(normalize(vec3(sub(sub(nGeo.member("x"), mul(hx, relief)), mul(fxg, reliefFine)), sub(sub(nGeo.member("y"), mul(hy, relief)), mul(fyg, reliefFine)), nGeo.member("z"))), "n");
			const sharp = local(clamp(u.sharpness, 0, 1), "sharp");
			const R = local(reflect(vec3(0, 0, 1), n), "R");
			const reflCol = local(mul(waterSky(R.member("x"), R.member("y"), lx, ly, envC, envS, sharp, "refl"), u.reflection), "reflCol");
			const thickness = local(mul(opticalThickness(field, frame), u.depth), "thickness");
			const depthT = local(clamp(mul(thickness, 1.3), 0, 1), "depthT");
			const wc = local(u.waterColor.member("rgb"), "wc");
			const transmit = local(beerLambert(thickness, vec3(mul(sub(1, wc.member("x")), 2.2), mul(sub(1, wc.member("y")), 2.2), mul(sub(1, wc.member("z")), 2.2))), "transmit");
			const shallowsC = clamp(u.shallows, 0, 1);
			const bodyTint = local(add(mul(mix(mix(wc, vec3(1, 1, 1), splat3(shallowsC)), wc, splat3(depthT)), transmit), mul(wc, mul(depthT, .18))), "bodyTint");
			const kl = keyLightAt(light, -.7);
			const diffuse = local(add(mul(pow(clamp(add(mul(dot(n, kl.member("L")), .5), .5), 0, 1), .8), .5), .5), "diffuse");
			const tDir = local(refract(vec3(0, 0, 1), n, float(1 / 1.333)), "tDir");
			const transSky = mul(waterSky(tDir.member("x"), tDir.member("y"), lx, ly, envC, envS, sharp, "trans"), transmit);
			const clarityT = mul(clamp(u.clarity, 0, 1), sub(1, mul(depthT, .8)));
			const body0 = mul(mix(bodyTint, add(mul(transSky, .6), mul(bodyTint, .4)), splat3(clarityT)), diffuse);
			const causT = local(mul(t, 3), "causT");
			const causP = local(add(mul(vec2(fx, fy), u.causticScale), vec2(mul(sin(mul(causT, .15)), .4), mul(mul(causT, .3), -1))), "causP");
			const c1 = sub(1, abs(waterFbm(causP, "fbmC1")));
			const c2 = sub(1, abs(waterFbm(add(mul(causP, 1.9), vec2(3.7, mul(mul(causT, .18), -1))), "fbmC2")));
			const causV = local(pow(clamp(add(mul(c1, .6), mul(c2, .4)), 0, 1), 3), "causV");
			const causMask = local(mul(add(mul(depthT, .8), .2), u.caustics), "causMask");
			const surface0 = mix(add(mul(body0, add(1, mul(mul(causV, causMask), .7))), mul(vec3(.55, .82, 1), mul(mul(causV, causMask), .45))), reflCol, splat3(clamp(schlickFresnel(grazingFlat(n), { gain: .96 }), 0, 1)));
			const spec = sharpGlint(clamp(dot(n, kl.member("H")), 0, 1), sharp, vec4(40, 600, .3, 1.3));
			const surface1 = add(surface0, mul(vec3(1, .98, .92), spec));
			const foamMask = local(mul(clamp(add(smoothstep(.62, .92, clamp(add(mul(h0, .5), .5), 0, 1)), mul(sub(1, smoothstep(0, .045, mul(sdf, -1))), .7)), 0, 1), u.foam), "foamMask");
			const surface2 = local(mix(surface1, vec3(1, 1, 1), splat3(mul(foamMask, .85))), "waterRgb");
			const transition = silhouette(field, u.edgeSoftness);
			return guarded(insideShape(sdf, pxH), vec4(surface2, transition), ZERO, "water");
		}
	})
});
var Water_default = componentDefinition;
export { componentDefinition as n, Water_default as t };
