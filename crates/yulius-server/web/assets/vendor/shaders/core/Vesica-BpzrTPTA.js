import { Ac as defineStd, _s as vesica } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Vesica",
	description: "Vesica piscis (lens shape) formed by the intersection of two overlapping circles",
	shape: {
		distance: ({ x, y }, u) => vesica(x, y, u("radius"), u("spread")),
		colorDescription: "Fill color of the vesica",
		centerDescription: "Center position of the vesica",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .35,
				description: "Radius of the two overlapping circles",
				ui: {
					type: "range",
					min: .05,
					max: 1,
					step: .01,
					label: "Size",
					group: "Shape"
				}
			},
			spread: {
				default: .5,
				description: "Circle separation — 0 = full circle overlap, 1 = infinitely thin lens",
				ui: {
					type: "range",
					min: .05,
					max: .95,
					step: .01,
					label: "Spread",
					group: "Shape"
				}
			}
		}
	}
});
var Vesica_default = componentDefinition;
export { componentDefinition as n, Vesica_default as t };
