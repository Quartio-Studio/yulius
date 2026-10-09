import { Ac as defineStd, is as crescent } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Crescent",
	description: "Crescent moon shape — an outer circle with an inner circle subtracted",
	shape: {
		distance: ({ x, y }, u) => crescent(x, y, u("radius"), u("innerRatio"), u("offset")),
		colorDescription: "Fill color of the crescent",
		centerDescription: "Center position of the crescent",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: {
			radius: {
				default: .3,
				description: "Outer circle radius",
				ui: {
					type: "range",
					min: .05,
					max: 1,
					step: .01,
					label: "Size",
					group: "Shape"
				}
			},
			innerRatio: {
				default: .8,
				description: "Inner (bite) circle radius as a fraction of outer radius",
				ui: {
					type: "range",
					min: .3,
					max: 1.2,
					step: .01,
					label: "Bite Size",
					group: "Shape"
				}
			},
			offset: {
				default: .2,
				description: "Horizontal distance the bite circle is shifted from center",
				ui: {
					type: "range",
					min: .01,
					max: .5,
					step: .01,
					label: "Bite Offset",
					group: "Shape"
				}
			}
		}
	}
});
var Crescent_default = componentDefinition;
export { componentDefinition as n, Crescent_default as t };
