import { Ac as defineStd, Ml as p, vn as pixelate } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Pixelate",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Pixelation effect with adjustable cell size",
	props: {
		scale: {
			default: 50,
			description: "Number of pixels along the longest edge (higher = smaller pixels)",
			ui: {
				type: ["range", "map"],
				min: 1,
				max: 200,
				step: 1,
				label: "Scale",
				group: "Effect"
			}
		},
		gap: {
			default: 0,
			description: "Space between pixels as a fraction of cell size (0 = no gap, 1 = fully invisible)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: .95,
				step: .01,
				label: "Gap",
				group: "Effect"
			}
		},
		roundness: {
			default: 0,
			description: "Roundness of each pixel's corners (0 = square, 1 = circle)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Roundness",
				group: "Effect"
			}
		}
	},
	effect: pixelate({
		scale: p("scale"),
		gap: p("gap"),
		roundness: p("roundness")
	})
});
var Pixelate_default = componentDefinition;
export { componentDefinition as n, Pixelate_default as t };
