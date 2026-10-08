import { Ac as defineStd, Ml as p, qt as bokehDefocus } from "./std-ClftfWpF.js";
import { r as transformAngle } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "BokehBlur",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Blurs",
	description: "Photographic lens blur where bright highlights bloom into aperture-shaped discs",
	requiresRTT: true,
	requiresChild: true,
	props: {
		radius: {
			default: 50,
			description: "Defocus amount — how far the lens blur spreads",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Radius",
				group: "Effect"
			}
		},
		highlightGain: {
			default: 4,
			description: "How strongly bright highlights bloom into discs",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 10,
				step: .1,
				label: "Highlight Bloom",
				group: "Highlights"
			}
		},
		highlightThreshold: {
			default: .6,
			description: "Brightness above which a highlight forms a disc",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Highlight Threshold",
				group: "Highlights"
			}
		},
		bladeShape: {
			default: "blades",
			description: "Aperture shape the highlights bloom into — bladed iris or a novelty cut-out (heart, star, …)",
			ui: {
				type: "select",
				options: [
					{
						label: "Blades",
						value: "blades"
					},
					{
						label: "Circle",
						value: "circle"
					},
					{
						label: "Star",
						value: "star"
					},
					{
						label: "Heart",
						value: "heart"
					},
					{
						label: "Flower",
						value: "flower"
					},
					{
						label: "Cross",
						value: "cross"
					},
					{
						label: "Ring",
						value: "ring"
					}
				],
				label: "Blade Shape",
				group: "Aperture"
			}
		},
		bladeCount: {
			default: 6,
			description: "Blade or point count for the Blades, Star and Flower shapes — 0–2 blades is a circular iris",
			ui: {
				type: "range",
				min: 0,
				max: 9,
				step: 1,
				label: "Aperture Blades",
				group: "Aperture"
			}
		},
		bladeRotation: {
			default: 0,
			transform: transformAngle,
			description: "Rotation of the aperture shape (in degrees)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Blade Rotation",
				group: "Aperture"
			}
		},
		chromaticFringe: {
			default: .2,
			description: "Lens color fringing on the disc edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Chromatic Fringe",
				group: "Aperture"
			}
		}
	},
	...bokehDefocus({
		radius: p("radius"),
		gain: p("highlightGain"),
		threshold: p("highlightThreshold"),
		shape: p("bladeShape"),
		blades: p("bladeCount"),
		rotation: p("bladeRotation"),
		fringe: p("chromaticFringe")
	})
});
var BokehBlur_default = componentDefinition;
export { componentDefinition as n, BokehBlur_default as t };
