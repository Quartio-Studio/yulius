import { Ac as defineStd, Cs as recompileWhen, Ml as p, an as fluteHighlight, in as fluteFrame, sn as gatherStack, xn as refractedTaps } from "./std-ClftfWpF.js";
import { a as transformColor, s as transformEdges } from "./transformations-P50Uo_Ju.js";
var flute = fluteFrame({
	shape: p("shape"),
	angle: p("angle"),
	frequency: p("frequency"),
	softness: p("softness"),
	waveAmplitude: p("waveAmplitude"),
	waveFrequency: p("waveFrequency"),
	refraction: p("refraction"),
	aberration: p("aberration")
});
const componentDefinition = defineStd({
	name: "FlutedGlass",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Distortions",
	description: "Full-screen fluted glass effect — refracts content through repeating cylindrical bars",
	animatedTime: { speed: "speed" },
	props: {
		shape: {
			default: "bars",
			description: "Cross-section shape of each flute",
			compileTime: true,
			transform: (value) => ({
				"bars": 0,
				"rounded": 1,
				"waves": 2
			})[value] ?? 0,
			ui: {
				type: "select",
				options: [
					{
						label: "Bars",
						value: "bars"
					},
					{
						label: "Rounded",
						value: "rounded"
					},
					{
						label: "Waves",
						value: "waves"
					}
				],
				label: "Shape",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			description: "Direction of the flutes in degrees (0 = vertical bars)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect"
			}
		},
		frequency: {
			default: 10,
			description: "Number of flutes across the longest viewport axis",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 20,
				step: 1,
				label: "Frequency",
				group: "Effect"
			}
		},
		softness: {
			default: .5,
			description: "How smoothly distortion fades from each flute centre to its edge (0 = flat middle / sharp seams, 1 = gentle curve)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Softness",
				group: "Effect"
			}
		},
		waveAmplitude: {
			default: .06,
			description: "How far each flute sways horizontally as it travels (Waves shape only)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .5,
				step: .01,
				label: "Wave Amplitude",
				group: "Effect",
				condition: { shape: "waves" }
			}
		},
		waveFrequency: {
			default: 1.5,
			description: "How many sways fit along each flute (Waves shape only)",
			ui: {
				type: ["range", "map"],
				min: .1,
				max: 10,
				step: .1,
				label: "Wave Frequency",
				group: "Effect",
				condition: { shape: "waves" }
			}
		},
		speed: {
			default: 0,
			description: "Animation speed — drifts the flute pattern over time and flows wave perturbations",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Speed",
				group: "Animation"
			}
		},
		refraction: {
			default: 1.5,
			description: "How aggressively each flute bends content beneath it",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 4,
				step: .01,
				label: "Refraction",
				group: "Effect"
			}
		},
		aberration: {
			default: .2,
			description: "Chromatic aberration — splits RGB along the refraction direction at flute seams",
			recompile: recompileWhen((prev, next) => prev > 0 !== next > 0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Aberration",
				group: "Effect"
			}
		},
		lightAngle: {
			default: 30,
			description: "Direction the light source is coming from (0 = head-on, 90 = grazing)",
			ui: {
				type: "range",
				min: -90,
				max: 90,
				step: 1,
				label: "Light Angle",
				group: "Highlight"
			}
		},
		highlight: {
			default: .2,
			description: "Strength of the specular reflection on each flute",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 2,
				step: .01,
				label: "Highlight",
				group: "Highlight"
			}
		},
		highlightSoftness: {
			default: .3,
			description: "Spread of the specular peak (0 = pin-tight, 1 = broad sheen)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Highlight Softness",
				group: "Highlight"
			}
		},
		highlightColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "Color of the specular highlight",
			ui: {
				type: "color",
				label: "Highlight Color",
				group: "Highlight"
			}
		},
		edges: {
			default: "mirror",
			description: "How to handle edges when distortion samples beyond the canvas",
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
	effect: gatherStack(refractedTaps(flute, {
		aberration: p("aberration"),
		edges: p("edges")
	}), [fluteHighlight(flute, {
		lightAngle: p("lightAngle"),
		highlight: p("highlight"),
		softness: p("highlightSoftness"),
		color: p("highlightColor")
	})], { resultAlpha: "straight" })
});
var FlutedGlass_default = componentDefinition;
export { componentDefinition as n, FlutedGlass_default as t };
