import { Ac as defineStd, Gt as bloom, Ml as p, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Glow",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Soft glow effect with adjustable intensity",
	requiresRTT: true,
	requiresChild: true,
	props: {
		intensity: {
			default: 1,
			description: "Glow intensity (brightness of the glow effect)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 50,
				step: .5,
				label: "Intensity",
				group: "Effect"
			}
		},
		threshold: {
			default: .5,
			description: "Brightness threshold for glow extraction (lower = more glow)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Threshold",
				group: "Effect"
			}
		},
		size: {
			default: 25,
			description: "Glow spread in pixels (clean up to ~72px, mild banding above)",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Glow Size",
				group: "Effect"
			}
		}
	},
	...bloom({
		intensity: p("intensity"),
		threshold: p("threshold"),
		size: p("size")
	})
});
var Glow_default = componentDefinition;
export { componentDefinition as n, Glow_default as t };
