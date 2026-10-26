import { act, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { usePatternViewport } from "./usePatternViewport";
let width=200, resize=()=>{};
function Fixture({ latest=48, focused=false }) {
 const host=useRef<HTMLDivElement>(null), view=usePatternViewport(host,48,latest,focused);
 return <><div aria-label="steps" ref={el=>{host.current=el;if(el)Object.defineProperties(el,{clientWidth:{configurable:true,get:()=>width},scrollWidth:{configurable:true,get:()=>48*view.columnWidth}});}} onScroll={view.onScroll}/>
 <output>{view.columnWidth}</output><button onClick={view.follow}>Follow</button></>;
}
afterEach(()=>{vi.unstubAllGlobals();width=200;});
it("fills wide viewports and follows new steps through narrowing, but never steals a held view or focus",()=>{
 vi.stubGlobal('ResizeObserver',class { constructor(callback:()=>void){resize=callback;}observe(){}disconnect(){}});
 const view=render(<Fixture/>), host=screen.getByLabelText('steps');
 expect(host.scrollLeft).toBe(1912);
 host.scrollLeft=0;fireEvent.scroll(host);view.rerender(<Fixture latest={49}/>);expect(host.scrollLeft).toBe(0);
 fireEvent.click(screen.getByRole('button',{name:'Follow'}));expect(host.scrollLeft).toBe(1912);
 view.rerender(<Fixture latest={50} focused/>);host.scrollLeft=400;view.rerender(<Fixture latest={51} focused/>);expect(host.scrollLeft).toBe(400);
 view.rerender(<Fixture latest={51}/>);expect(host.scrollLeft).toBe(1912);
 width=4800;act(()=>resize());expect(screen.getByRole('status').textContent).toBe('100');expect(host.scrollLeft).toBe(0);
 width=400;act(()=>resize());expect(host.scrollLeft).toBe(1712);
});
