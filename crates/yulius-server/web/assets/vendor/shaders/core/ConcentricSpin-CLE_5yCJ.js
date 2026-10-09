import { Ac as defineStd, Ml as p, Nl as centerPropConfig, Pl as edgesPropConfig, Vn as concentricRings } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "ConcentricSpin",
	role: "warp",
	category: "Distortions",
	description: "Concentric rings that each rotate the underlying image by different amounts",
	boundingBoxDeclaration: { aspectRatio: null },
	animatedTime: { speed: "speed" },
	props: {
		intensity: {
			default: 20,
			description: "Maximum rotation angle per ring",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Intensity",
				group: "Effect"
			}
		},
		rings: {
			default: 8,
			description: "Number of concentric rings",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 30,
				step: 1,
				label: "Rings",
				group: "Effect"
			}
		},
		smoothness: {
			default: .03,
			description: "Softness of transitions between rings",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Smoothness",
				group: "Effect"
			}
		},
		seed: {
			default: 0,
			description: "Randomization seed for per-ring rotation variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		speed: {
			default: .1,
			description: "Speed of continuous ring rotation",
			ui: {
				type: "range",
				min: -5,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		speedRandomness: {
			default: .5,
			description: "How much each ring varies in rotation speed and direction",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Speed Randomness",
				group: "Animation"
			}
		},
		edges: edgesPropConfig("mirror", "How to handle edges when distortion pushes content out of bounds"),
		center: centerPropConfig("Center point of the concentric rings", { label: "Center Position" })
	},
	map: concentricRings({
		center: p("center"),
		intensity: p("intensity"),
		rings: p("rings"),
		smoothness: p("smoothness"),
		seed: p("seed"),
		speedRandomness: p("speedRandomness")
	}),
	missingChildMessage: "You must pass a child component into the ConcentricSpin shader."
});
var ConcentricSpin_default = componentDefinition;
export { componentDefinition as n, ConcentricSpin_default as t };
