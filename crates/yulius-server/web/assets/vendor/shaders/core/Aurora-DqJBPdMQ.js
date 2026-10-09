import { Ac as defineStd, Al as uniformOf, Dc as vec3, Dr as layered, Fa as rayBands, Gs as clamp, Ic as animatedTime, Is as softBand, Ml as p, Oc as vec4, Oo as colorLadder3, Sc as splat3, Tc as sub, Ws as add, Ys as exp, bc as smoothstep, ca as wavyLine, cc as max, dc as mul, dr as surfaceOf, ji as volumeNoiseAt, mc as pow, oc as local, qs as div, rc as hashPhase, uc as mix } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var ARC_WAVES = [
	{
		freq: .7,
		speed: .4,
		amount: .15,
		salt: {
			k: 12.9,
			big: 4758.5
		}
	},
	{
		freq: 1.6,
		speed: .65,
		amount: .08,
		salt: {
			k: 78.2,
			big: 2847.1
		}
	},
	{
		freq: 2.9,
		speed: .3,
		amount: .04,
		salt: {
			k: 41.6,
			big: 1593.7
		}
	}
];
var RAY_SALT = {
	k: 53.7,
	big: 3847.2
};
var LAYERS = [
	{
		timeOffset: 0,
		weight: 1
	},
	{
		timeOffset: 3.1,
		weight: .65
	},
	{
		timeOffset: 6.7,
		weight: .45
	},
	{
		timeOffset: 9.4,
		weight: .3
	}
];
const componentDefinition = defineStd({
	name: "Aurora",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Mesmerizing aurora borealis with layered curtains, vertical rays, and flowing light.",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#a533f8",
			transform: transformColor,
			description: "Edge color at the curtain base",
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#22ee88",
			transform: transformColor,
			description: "Core color in the bright center",
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		colorC: {
			default: "#1694e8",
			transform: transformColor,
			description: "Tip color at the ray ends",
			ui: {
				type: "color",
				label: "Color C",
				group: "Colors"
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
		},
		balance: {
			default: 50,
			description: "Shifts color distribution across the curtain height",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Balance",
				group: "Colors"
			}
		},
		intensity: {
			default: 80,
			description: "Overall aurora brightness",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Aurora"
			}
		},
		curtainCount: {
			default: 4,
			compileTime: true,
			description: "Number of aurora curtain layers",
			ui: {
				type: "range",
				min: 1,
				max: 4,
				step: 1,
				label: "Curtains",
				group: "Aurora"
			}
		},
		speed: {
			default: 5,
			description: "Animation speed",
			ui: {
				type: "range",
				min: -10,
				max: 10,
				step: .1,
				label: "Speed",
				group: "Aurora"
			}
		},
		waviness: {
			default: 50,
			description: "How much the curtains undulate",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 200,
				step: 1,
				label: "Waviness",
				group: "Aurora"
			}
		},
		rayDensity: {
			default: 20,
			description: "Density of vertical ray structures",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Ray Detail",
				group: "Aurora"
			}
		},
		height: {
			default: 120,
			description: "How tall the aurora extends",
			ui: {
				type: ["range", "map"],
				min: 10,
				max: 200,
				step: 1,
				label: "Height",
				group: "Scene"
			}
		},
		center: {
			default: {
				x: .5,
				y: 0
			},
			transform: transformPosition,
			description: "Center position of the aurora",
			ui: {
				type: "position",
				label: "Center",
				group: "Scene"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for variation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Aurora"
			}
		}
	},
	paint: (params) => {
		const { uv, viewport } = surfaceOf(params);
		const aspect = local(div(viewport.member("x"), max(viewport.member("y"), 1e-6)), "aspect");
		const t = local(animatedTime(params), "auroraTime");
		const seed = uniformOf(p("seed"), params);
		const center = uniformOf(p("center"), params);
		const centerX = center.member("x");
		const centerY = sub(1, center.member("y"));
		const wav = mul(uniformOf(p("waviness"), params), .01);
		const rayAmt = mul(uniformOf(p("rayDensity"), params), .01);
		const auroraHeight = mul(uniformOf(p("height"), params), .01);
		const intens = mul(uniformOf(p("intensity"), params), .018);
		const balanceExp = mix(2.5, .4, mul(uniformOf(p("balance"), params), .01));
		const curtain = (layerIdx, timeOffset) => {
			const tSlow = local(add(mul(t, .06), timeOffset), "tSlow");
			const lSeed = local(add(seed, Math.fround(layerIdx * Math.fround(31.7))), "lSeed");
			const wx = local(mul(mul(sub(uv.member("x"), centerX), aspect), 2), "wx");
			const arc = wavyLine({
				along: wx,
				time: tSlow,
				waves: ARC_WAVES.map((w) => ({
					...w,
					phase: hashPhase(lSeed, w.salt)
				}))
			});
			const noiseWarp = local(volumeNoiseAt(vec3(mul(wx, .5), mul(tSlow, .25), mul(lSeed, .1))), "curtainNoise");
			const pathY = add(add(centerY, mul(arc, wav)), mul(mul(noiseWarp, .14), wav));
			const rays = local(rayBands({
				along: add(wx, mul(noiseWarp, 2.5)),
				frequency: mul(mix(6, 20, rayAmt), .15),
				time: tSlow,
				speed: .1,
				phase: hashPhase(lSeed, RAY_SALT),
				sharpness: 1.5
			}), "curtainRays");
			const localHeight = mul(auroraHeight, mix(.4, 1, rays));
			const distFromPath = local(sub(uv.member("y"), pathY), "distFromPath");
			const topExtent = local(mul(localHeight, add(mul(rays, .4), .6)), "topExtent");
			const envelope = softBand({
				distance: distFromPath,
				below: {
					from: -.04,
					to: .015
				},
				above: { to: topExtent }
			});
			const colorT = local(clamp(div(distFromPath, add(topExtent, .001)), 0, 1), "colorT");
			const sectionBright = mix(.4, 1, add(mul(noiseWarp, .5), .5));
			const coreD = sub(colorT, .2);
			const coreBright = add(mul(exp(mul(mul(coreD, coreD), -6)), .3), 1);
			return {
				colorT,
				alpha: local(mul(mul(mul(envelope, rays), sectionBright), coreBright), "curtainAlpha")
			};
		};
		const count = Math.max(1, Math.min(4, Math.round(params.propValues.curtainCount ?? 4)));
		const { weightedSum, totalWeight } = layered(LAYERS.slice(0, count), ({ timeOffset, weight }, layerIdx) => {
			const { colorT, alpha } = curtain(layerIdx, timeOffset);
			return {
				value: colorT,
				weight: weight === 1 ? alpha : mul(alpha, weight)
			};
		});
		const total = local(totalWeight, "auroraAlpha");
		const avgT = local(pow(div(weightedSum, add(total, .001)), balanceExp), "auroraAvgT");
		const color = colorLadder3({
			a: p("colorA"),
			b: p("colorB"),
			c: p("colorC"),
			space: p("colorSpace")
		})(smoothstep(0, .2, avgT), smoothstep(.4, .85, avgT), params);
		const brightness = local(mul(total, intens), "auroraGlow");
		return vec4(clamp(mul(color.member("rgb"), brightness), splat3(0), splat3(1)), clamp(mul(brightness, .5), 0, 1));
	}
});
var Aurora_default = componentDefinition;
export { componentDefinition as n, Aurora_default as t };
