import { Ac as defineStd, Ml as p, Un as displaceByLayer } from "./std-ClftfWpF.js";
import { s as transformEdges } from "./transformations-P50Uo_Ju.js";
var CHANNEL_MODES = {
	twoAxis: 0,
	directional: 1
};
const componentDefinition = defineStd({
	name: "DisplacementMap",
	role: "filter",
	species: "gather",
	category: "Distortions",
	description: "Distorts child content using another layer's pixels as a displacement map",
	boundingBoxDeclaration: { aspectRatio: null },
	props: {
		source: {
			default: "",
			description: "The layer whose pixels drive the displacement — its red/green (or luminance) push the content around",
			compileTime: true,
			ui: {
				type: "layer",
				label: "Source Layer",
				group: "Effect"
			}
		},
		amount: {
			default: .3,
			description: "Overall displacement strength",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Amount",
				group: "Effect"
			}
		},
		channelMode: {
			default: "twoAxis",
			description: "How the source drives the push — red→X / green→Y, or luminance along a fixed angle",
			compileTime: true,
			transform: (value) => CHANNEL_MODES[value] ?? 0,
			ui: {
				type: "select",
				options: [{
					label: "Red / Green",
					value: "twoAxis"
				}, {
					label: "Luminance",
					value: "directional"
				}],
				label: "Channels",
				group: "Effect"
			}
		},
		angle: {
			default: 0,
			description: "Push direction in degrees (used by the Luminance channel mode)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 360,
				step: 1,
				label: "Angle",
				group: "Effect",
				condition: { channelMode: "directional" }
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
	effect: displaceByLayer({
		source: p("source"),
		amount: p("amount"),
		channels: p("channelMode"),
		angle: p("angle"),
		edges: p("edges")
	})
});
var DisplacementMap_default = componentDefinition;
export { componentDefinition as n, DisplacementMap_default as t };
