import { Ac as defineStd, cs as heart } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	role: "shape",
	name: "Heart",
	description: "Heart shape with adjustable size",
	shape: {
		distance: ({ x, y }, u) => heart(x, y, u("radius")),
		colorDescription: "Fill color of the heart",
		centerDescription: "Center position of the heart",
		bounds: { size: {
			width: "radius",
			height: "radius"
		} },
		shapeProps: { radius: {
			default: .32,
			description: "Size of the heart (fit radius)",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Size",
				group: "Shape"
			}
		} }
	}
});
var Heart_default = componentDefinition;
export { componentDefinition as n, Heart_default as t };
