import { Ac as defineStd, Al as uniformOf, Gs as clamp, Ic as animatedTime, Ml as p, No as hueWheel, Oc as vec4, Sc as splat3, Tc as sub, Ws as add, Ys as exp, bc as smoothstep, cc as max, dc as mul, dr as surfaceOf, ec as gaussBell, fc as neg, oc as local, qs as div, uc as mix, ur as segmentFrame, xc as softPlus } from "./std-ClftfWpF.js";
import { a as transformColor, c as transformPosition } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "Prism",
	role: "generator",
	category: "Textures",
	description: "A beam of light that fans out and splits into a slowly-rotating rainbow past a controllable point. Transparent background — composite over a dark layer.",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		position: {
			default: {
				x: .18,
				y: .18
			},
			transform: transformPosition,
			description: "Where the beam originates",
			ui: {
				type: "position",
				label: "Source",
				group: "Beam"
			}
		},
		beamWidth: {
			default: .04,
			description: "Thickness of the white beam",
			ui: {
				type: ["range", "map"],
				min: .002,
				max: .3,
				step: .002,
				label: "Beam Width",
				group: "Beam"
			}
		},
		intensity: {
			default: 1.6,
			description: "Brightness (above 1 blows the core to white)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 4,
				step: .01,
				label: "Intensity",
				group: "Beam"
			}
		},
		beamColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "color of the beam before it splits",
			ui: {
				type: "color",
				label: "Beam Color",
				group: "Beam"
			}
		},
		startFalloff: {
			default: .15,
			description: "How softly the beam fades in at its source",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Start Falloff",
				group: "Beam"
			}
		},
		endFalloff: {
			default: .6,
			description: "How quickly the rainbow fades out with distance",
			ui: {
				type: ["range", "map"],
				min: .02,
				max: 2,
				step: .01,
				label: "End Falloff",
				group: "Beam"
			}
		},
		splitPosition: {
			default: {
				x: .5,
				y: .42
			},
			transform: transformPosition,
			description: "Where the beam starts to fan out and split (projected onto the beam axis)",
			ui: {
				type: "position",
				label: "Split Point",
				group: "Spectrum"
			}
		},
		spread: {
			default: .7,
			description: "How widely the rainbow diffuses as it travels past the split point",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .01,
				label: "Spread",
				group: "Spectrum"
			}
		},
		softness: {
			default: .12,
			description: "How quickly the white beam transitions into the rainbow",
			ui: {
				type: ["range", "map"],
				min: .001,
				max: 1,
				step: .005,
				label: "Softness",
				group: "Spectrum"
			}
		},
		saturation: {
			default: .95,
			description: "Rainbow saturation — 0 = white, 1 = full color",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Saturation",
				group: "Spectrum"
			}
		},
		speed: {
			default: .1,
			description: "Speed the hue slowly rotates through the rainbow (0 = static)",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Hue Speed",
				group: "Spectrum"
			}
		}
	},
	paint: (params) => {
		const u = (name) => uniformOf(p(name), params);
		const beam = segmentFrame({
			from: p("position"),
			to: p("splitPosition")
		})(params, surfaceOf(params));
		const soft = local(max(u("softness"), .001), "soft");
		const past = local(softPlus(sub(beam.along, beam.length), mul(soft, .6)), "past");
		const longEnv = local(mul(smoothstep(0, max(u("startFalloff"), .001), beam.along), exp(neg(div(past, max(u("endFalloff"), .001))))), "longEnv");
		const fanWidth = max(add(u("beamWidth"), mul(past, u("spread"))), .001);
		const an = local(div(beam.across, fanWidth), "an");
		const profile = gaussBell(an, 2.5);
		const splitSat = smoothstep(beam.length, add(beam.length, soft), beam.along);
		const hue = add(mul(an, .42), animatedTime(params));
		const col = mix(u("beamColor").member("rgb"), hueWheel(hue), splat3(mul(splitSat, u("saturation"))));
		const amt = local(mul(profile, longEnv), "amt");
		return vec4(local(mul(mul(col, amt), u("intensity")), "beamRgb").member("xyz"), clamp(amt, 0, 1));
	}
});
var Prism_default = componentDefinition;
export { componentDefinition as n, Prism_default as t };
