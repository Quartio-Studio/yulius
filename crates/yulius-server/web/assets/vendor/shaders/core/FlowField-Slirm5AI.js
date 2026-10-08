import { Ac as defineStd, Gn as flowNoise, Ml as p } from "./std-ClftfWpF.js";
import { s as transformEdges } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "FlowField",
	role: "warp",
	category: "Distortions",
	description: "Fluid-like distortion with constant smooth motion",
	boundingBoxDeclaration: { aspectRatio: null },
	animatedTime: { speed: "speed" },
	extraAnimatedTimes: { evolution: "evolutionSpeed" },
	props: {
		strength: {
			default: .15,
			description: "Intensity of the flow distortion",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .5,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		detail: {
			default: 2,
			description: "Scale of the flow patterns",
			ui: {
				type: ["range", "map"],
				min: .5,
				max: 5,
				step: .1,
				label: "Detail",
				group: "Effect"
			}
		},
		speed: {
			default: 0,
			description: "Speed of the flow",
			ui: {
				type: "range",
				min: 0,
				max: 20,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		evolutionSpeed: {
			default: 0,
			description: "How fast the flow field pattern reshapes over time",
			ui: {
				type: "range",
				min: 0,
				max: 20,
				step: .1,
				label: "Evolution Speed",
				group: "Animation"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for flow pattern variation",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		},
		edges: {
			default: "mirror",
			description: "How to handle edges when distortion pushes content out of bounds",
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
	map: flowNoise({
		strength: p("strength"),
		detail: p("detail"),
		seed: p("seed")
	})
});
var FlowField_default = componentDefinition;
export { componentDefinition as n, FlowField_default as t };
